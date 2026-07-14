//! **SPIKE de P2.b — transpile a Rust** (evaluación de rendimiento, jul 2026).
//!
//! Prueba de concepto: emite código Rust para un SUBCONJUNTO de raylang (funciones, `int`/`float`/
//! `bool`, aritmética, `if`/`while`/`for`-rango, `let`/`var`, recursión, `print`) — suficiente para
//! `fib`/`loopsum`, el peor caso de la VM. El objetivo es MEDIR el techo del codegen nativo, no cubrir
//! el lenguaje. Todo nodo no soportado → `Err` claro (el spike es honesto sobre su alcance).
//!
//! Semántica del spike: aritmética `int` con **wrapping** (operadores Rust nativos; en release
//! `overflow-checks=off`), NO checked como la VM. Fiel para programas sin desbordamiento (fib/loopsum);
//! un transpilador de producción decidiría la política de overflow. Erasure total (sin GC, sin heap).

use crate::ast::{BinaryOp, Block, Expr, ExprKind, ForIter, ForPat, Function, Program, Stmt, StmtKind, Type, UnaryOp};

/// Transpila un programa (ya chequeado) a un `String` de código Rust autocontenido, o un error si usa
/// una construcción fuera del subconjunto del spike.
pub fn transpile(prog: &Program) -> Result<String, String> {
    let mut out = String::new();
    out.push_str("// Generado por el spike de transpile-a-Rust de raylang (P2.b).\n");
    out.push_str("#![allow(unused_parens, unused_mut, dead_code)]\n\n");

    let mut main_ret_int = false;
    let mut main_seen = false;
    let mut skipped = 0usize;
    for f in &prog.functions {
        // Saltamos las sintéticas (prelude manglado #/::/__) y las genéricas (map/filter/fold/… del
        // prelude): el subconjunto escalar no las usa. Si el programa del usuario llamara a una saltada,
        // rustc fallará con "función no encontrada" → el spike es honesto sobre su alcance.
        if f.name.contains('#')
            || f.name.contains("::")
            || f.name.starts_with("__")
            || !f.type_params.is_empty()
        {
            skipped += 1;
            continue;
        }
        let rust_name = if f.name == "main" { "ray_main".to_string() } else { f.name.clone() };
        // Emitimos a un buffer propio: si la función usa algo fuera del subconjunto (prelude no genérico
        // como `assert`), se salta sin corromper la salida.
        let mut fbuf = String::new();
        match emit_function(&mut fbuf, &rust_name, f) {
            Ok(()) => {
                out.push_str(&fbuf);
                out.push('\n');
                if f.name == "main" {
                    main_seen = true;
                    main_ret_int = matches!(f.return_type, Type::Int);
                }
            }
            Err(_) => skipped += 1,
        }
    }
    if !main_seen {
        return Err(format!("spike: `main` no está en el subconjunto soportado ({} fns saltadas)", skipped));
    }

    // Envoltorio `fn main`: el código de salida es el `int` que devuelve `main` (unit → 0).
    out.push_str("fn main() {\n");
    if main_ret_int {
        out.push_str("    std::process::exit(ray_main() as i32);\n");
    } else {
        out.push_str("    ray_main();\n");
    }
    out.push_str("}\n");
    Ok(out)
}

fn emit_function(out: &mut String, rust_name: &str, f: &Function) -> Result<(), String> {
    let params: Vec<String> = f
        .params
        .iter()
        .map(|p| Ok(format!("mut {}: {}", p.name, ty(&p.ty)?)))
        .collect::<Result<_, String>>()?;
    out.push_str(&format!("fn {}({}) -> {} ", rust_name, params.join(", "), ty(&f.return_type)?));
    emit_block(out, &f.body)?;
    out.push('\n');
    Ok(())
}

/// Un tipo de raylang → su equivalente Rust (solo el subconjunto escalar).
fn ty(t: &Type) -> Result<String, String> {
    Ok(match t {
        Type::Int => "i64",
        Type::Float => "f64",
        Type::Bool => "bool",
        Type::Unit => "()",
        other => return Err(format!("spike: tipo no soportado {:?}", other)),
    }
    .to_string())
}

fn emit_block(out: &mut String, b: &Block) -> Result<(), String> {
    out.push_str("{\n");
    for s in &b.statements {
        emit_stmt(out, s)?;
    }
    if let Some(tail) = &b.tail {
        out.push_str("    ");
        emit_expr(out, tail)?;
        out.push('\n');
    }
    out.push('}');
    Ok(())
}

fn emit_stmt(out: &mut String, s: &Stmt) -> Result<(), String> {
    out.push_str("    ");
    match &s.kind {
        StmtKind::Let { name, mutable, value, .. } => {
            // sin anotación de tipo: Rust la infiere (los literales int emiten `i64`).
            out.push_str(if *mutable { "let mut " } else { "let " });
            out.push_str(name);
            out.push_str(" = ");
            emit_expr(out, value)?;
            out.push_str(";\n");
        }
        StmtKind::Assign { target, value } => {
            emit_expr(out, target)?;
            out.push_str(" = ");
            emit_expr(out, value)?;
            out.push_str(";\n");
        }
        StmtKind::Return { value } => {
            out.push_str("return");
            if let Some(v) = value {
                out.push(' ');
                emit_expr(out, v)?;
            }
            out.push_str(";\n");
        }
        StmtKind::Expr(e) => {
            emit_expr(out, e)?;
            out.push_str(";\n");
        }
        StmtKind::For { pat, iter, body } => {
            let var = match pat {
                ForPat::Single(n) => n.clone(),
                ForPat::Tuple(_) => return Err("spike: for sobre tupla (Map) no soportado".into()),
            };
            match iter {
                ForIter::Range { start, end } => {
                    out.push_str(&format!("for {} in ", var));
                    emit_expr(out, start)?;
                    out.push_str("..");
                    emit_expr(out, end)?;
                    out.push(' ');
                    emit_block(out, body)?;
                    out.push('\n');
                }
                _ => return Err("spike: for sobre colección/iterador no soportado".into()),
            }
        }
        other => return Err(format!("spike: sentencia no soportada {:?}", other)),
    }
    Ok(())
}

fn emit_expr(out: &mut String, e: &Expr) -> Result<(), String> {
    match &e.kind {
        ExprKind::Int(n) => out.push_str(&format!("{}i64", n)),
        ExprKind::Float(x) => out.push_str(&format!("{:?}f64", x)),
        ExprKind::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        ExprKind::Ident(name) => out.push_str(name),
        ExprKind::Unary { op, expr } => {
            out.push('(');
            out.push_str(match op {
                UnaryOp::Neg => "-",
                UnaryOp::Not => "!",
                UnaryOp::BitNot => "!",
            });
            emit_expr(out, expr)?;
            out.push(')');
        }
        ExprKind::Binary { op, left, right } => {
            out.push('(');
            emit_expr(out, left)?;
            out.push_str(&format!(" {} ", binop(*op)));
            emit_expr(out, right)?;
            out.push(')');
        }
        ExprKind::Call { callee, args } => {
            // print(x) → println!("{}", x); resto → llamada nombrada ordinaria.
            if let ExprKind::Ident(name) = &callee.kind {
                if name == "print" {
                    out.push_str("println!(\"{}\", ");
                    emit_expr(out, &args[0])?;
                    out.push(')');
                    return Ok(());
                }
                out.push_str(name);
                out.push('(');
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    emit_expr(out, a)?;
                }
                out.push(')');
            } else {
                return Err("spike: llamada a expresión (no nombre) no soportada".into());
            }
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            out.push_str("if ");
            emit_expr(out, cond)?;
            out.push(' ');
            emit_block(out, then_branch)?;
            if let Some(eb) = else_branch {
                out.push_str(" else ");
                emit_expr(out, eb)?;
            }
        }
        ExprKind::While { cond, body } => {
            out.push_str("while ");
            emit_expr(out, cond)?;
            out.push(' ');
            emit_block(out, body)?;
        }
        ExprKind::Block(b) => emit_block(out, b)?,
        other => return Err(format!("spike: expresión no soportada {:?}", other)),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::transpile;

    fn transpile_src(src: &str) -> String {
        let tokens = crate::lexer::lex(src).expect("lex");
        let mut prog = crate::parser::parse(tokens).expect("parse");
        crate::checker::check(&mut prog).expect("check");
        transpile(&prog).expect("transpile")
    }

    #[test]
    fn transpila_fib_recursivo() {
        let rust = transpile_src(
            "fn fib(n: int) -> int { if (n < 2) { n } else { fib(n - 1) + fib(n - 2) } }\n\
             fn main() { print(fib(10)); }",
        );
        // La firma se traduce con tipos escalares y el cuerpo con la recursión intacta.
        assert!(rust.contains("fn fib(mut n: i64) -> i64"), "{}", rust);
        assert!(rust.contains("fib((n - 1i64))"), "{}", rust);
        assert!(rust.contains("println!(\"{}\", fib(10i64))"), "{}", rust);
        assert!(rust.contains("fn main()"), "{}", rust);
    }

    #[test]
    fn transpila_bucle_for_rango() {
        let rust = transpile_src(
            "fn main() { var acc: int = 0; for i in 0..100 { acc = acc + i; } print(acc); }",
        );
        assert!(rust.contains("for i in 0i64..100i64"), "{}", rust);
        assert!(rust.contains("let mut acc = 0i64"), "{}", rust);
    }

    #[test]
    fn rechaza_fuera_del_subconjunto() {
        // Un `main` que usa strings (fuera del subconjunto escalar) → el spike lo salta y no hay `main`.
        let tokens = crate::lexer::lex("fn main() { let s = \"hi\"; print(s); }").unwrap();
        let mut prog = crate::parser::parse(tokens).unwrap();
        crate::checker::check(&mut prog).unwrap();
        assert!(super::transpile(&prog).is_err());
    }
}

fn binop(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
        BinaryOp::BitAnd => "&",
        BinaryOp::BitOr => "|",
        BinaryOp::BitXor => "^",
        BinaryOp::Shl => "<<",
        BinaryOp::Shr => ">>",
    }
}
