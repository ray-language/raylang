//! Tests de `vm` (movimiento puro; usar `git log --follow`).

use super::*;
use crate::ast::Expr;
use crate::compiler::{compile_expr, compile_program};

fn expr_of(src: &str) -> Expr {
    let prog_src = format!("fn v() {{ {} }}", src);
    let tokens = crate::lexer::lex(&prog_src).expect("lex ok");
    let prog = crate::parser::parse(tokens).expect("parse ok");
    *prog.functions[0].body.tail.clone().expect("expression in tail position")
}

fn run_vm(src: &str) -> Value {
    let chunk = compile_expr(&expr_of(src)).expect("compila");
    run(&chunk).expect("ejecuta sin error")
}

/// Oráculo a nivel de expresión (int): VM vs intérprete.
fn oracle_int(src: &str) {
    let prog_src = format!("fn main() -> int {{ {} }}", src);
    let tokens = crate::lexer::lex(&prog_src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    crate::checker::check(&mut prog).expect("check ok");
    let interp = crate::interpreter::run(&prog).expect("intérprete ok");
    // La VM ejecuta el programa **ya chequeado** (no la expresión cruda): así se
    // aplican las bajadas del checker —UFCS/métodos— que la forma de método de los
    // builtins de contenedor (`s.len()`, `b.sub_bytes(...)`) necesita para compilar.
    let compiled = compile_program(&prog).expect("compila");
    let vm = run_program(&compiled).expect("vm ok");
    assert_eq!(interp, vm, "VM y intérprete difieren en `{}`", src);
}

/// **El oráculo a nivel de programa completo**: compila y ejecuta el programa
/// en la VM y en el intérprete, y exige que el resultado coincida.
fn oracle_program(src: &str) {
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    crate::checker::check(&mut prog).expect("check ok");
    let interp = crate::interpreter::run(&prog).expect("intérprete ok");
    let compiled = compile_program(&prog).expect("compila");
    let vm = run_program(&compiled).expect("vm ok");
    assert_eq!(interp, vm, "VM y intérprete difieren");
}

/// M64.1 (regresión): el `Return` que baja `?` ocurre en MITAD de una expresión, con operandos
/// pendientes en la pila. Sin truncar la pila a la base del marco, esos operandos huérfanos
/// desalineaban los argumentos de la siguiente llamada del llamador (aquí: `caso("b", …)` recibía
/// basura como `etiqueta` → ICE "combinación operador/operandos" o divergencia con el intérprete).
#[test]
fn pending_operands_after_try_err_oracle() {
    oracle_program(
        r#"
        fn fails() -> Result<int, string> { Result.Err("boom") }
        fn media(x: int) -> Result<int, string> {
            let v = x + fails()? * 2; // el `?` retorna con `x` pendiente en la pila
            Result.Ok(v)
        }
        fn case(label: string, r: Result<int, string>) -> string {
            match (r) {
                Result.Ok(v) => label + ": ok",
                Result.Err(e) => label + ": " + e,
            }
        }
        fn main() -> int {
            let a = case("a", media(1)); // la etiqueta queda pendiente mientras `media` falla por `?`
            let b = case("b", media(2)); // sin el fix, esta segunda llamada leía la pila corrida
            a.len() + b.len()
        }
        "#,
    );
}

/// M41: **FFI**. Llamar a funciones C nativas (libm/libc) por `dlopen`/`dlsym`. Determinista
/// (sqrt/pow/abs dan lo mismo siempre) → el oráculo VM↔intérprete vale: ambos motores llaman a la
/// MISMA función C y deben coincidir. Cubre float→float, aridad 2, e int→int (libc `abs`).
#[test]
fn ffi_libm_oracle() {
    oracle_program(
        "extern \"m\" {\n\
         \x20 fn sqrt(x: float) -> float;\n\
         \x20 fn pow(base: float, exp: float) -> float;\n\
         }\n\
         extern \"c\" {\n\
         \x20 fn abs(n: int) -> int;\n\
         }\n\
         fn main() -> int {\n\
         \x20 if (sqrt(16.0) == 4.0 && pow(2.0, 10.0) == 1024.0 && abs(0 - 5) == 5) { 42 } else { 0 }\n\
         }",
    );
}

/// M41.2: **FFI con string/bytes** → `char*`. Un `string` se marshala a una `CString`
/// NUL-terminada; un `bytes` se pasa por el puntero de su buffer. Determinista (strlen/atoi) →
/// oráculo. Programas separados porque el nombre extern ES el símbolo (un `strlen` por programa).
#[test]
fn ffi_strings_oracle() {
    // string → char*: strlen y atoi.
    oracle_program(
        "extern \"c\" {\n\
         \x20 fn strlen(s: string) -> int;\n\
         \x20 fn atoi(s: string) -> int;\n\
         }\n\
         fn main() -> int {\n\
         \x20 if (strlen(\"hello mundo\") == 10 && atoi(\"42\") == 42 && atoi(\"  -7x\") == 0 - 7) { 1 } else { 0 }\n\
         }",
    );
    // bytes → puntero al buffer (NUL-terminado a mano con un literal de bytes).
    oracle_program(
        "extern \"c\" {\n\
         \x20 fn strlen(s: bytes) -> int;\n\
         }\n\
         fn main() -> int {\n\
         \x20 if (strlen(b\"abcde\\x00\") == 5) { 1 } else { 0 }\n\
         }",
    );
}

/// M41.3: **FFI con retorno `char*`** → `Option<bytes>`/`Option<string>`. `strstr` es determinista
/// (devuelve un puntero DENTRO del argumento, o NULL si no encuentra) → oráculo. Some/None + el
/// azúcar de string, en ambos motores.
#[test]
fn ffi_char_ptr_return_oracle() {
    // Option<string>: encontrado → Some("world"); no encontrado → None.
    oracle_program(
        "extern \"c\" { fn strstr(h: string, n: string) -> Option<string>; }\n\
         fn d(o: Option<string>) -> int {\n\
         \x20 match (o) { Option.Some(s) => s.len(), Option.None => 0 - 1 }\n\
         }\n\
         fn main() -> int {\n\
         \x20 d(strstr(\"hello world\", \"world\")) * 10 + (d(strstr(\"abc\", \"z\")) + 1)\n\
         }",
    ); // "world"→len 5 ⇒ 50; no encontrado→-1 ⇒ +0 ⇒ 50
    // Option<bytes>: primitiva cruda.
    oracle_program(
        "extern \"c\" { fn strstr(h: string, n: string) -> Option<bytes>; }\n\
         fn main() -> int {\n\
         \x20 match (strstr(\"raylang\", \"lang\")) { Option.Some(b) => b.len(), Option.None => 0 }\n\
         }",
    ); // "lang" ⇒ len 4
}

/// M40.1a: **guardas** en los brazos del match (`patrón if <cond> => …`). El brazo casa solo si
/// el patrón liga Y la guarda es true; si no, se sigue al siguiente. Oráculo VM↔intérprete.
#[test]
fn match_guards_oracle() {
    // clasificar por rango: 3=grande, 2=positivo, 1=neg/cero, 0=nada. Un dígito por caso.
    let prog = "\
        fn c(o: Option<int>) -> int {\n\
        \x20 match (o) {\n\
        \x20   Option.Some(n) if n > 100 => 3,\n\
        \x20   Option.Some(n) if n > 0 => 2,\n\
        \x20   Option.Some(n) => 1,\n\
        \x20   Option.None => 0,\n\
        \x20 }\n\
        }\n\
        fn main() -> int {\n\
        \x20 c(Option.Some(500)) * 1000 + c(Option.Some(7)) * 100 + c(Option.Some(0 - 5)) * 10 + c(Option.None)\n\
        }";
    oracle_program(prog); // ambos motores → 3210
    // Guarda sobre un binding catch-all (no sobre una variante), y fallback tras ella.
    oracle_program("\
        fn f(o: Option<int>) -> int { match (o) { x if false => 9, _ => 1 } }\n\
        fn main() -> int { f(Option.Some(5)) + f(Option.None) }"); // 2
    // Guarda que usa **UFCS** (`xs.len()`): debe pasar por el lowering (M40.1a: los pases bajan
    // también la guarda, no solo el cuerpo).
    oracle_program("\
        fn g(o: Option<[int]>) -> int { match (o) { Option.Some(xs) if xs.len() > 2 => 1, Option.Some(xs) => 0, Option.None => 0 - 1 } }\n\
        fn main() -> int { g(Option.Some([1, 2, 3])) * 100 + (g(Option.Some([9])) + 5) * 10 }"); // 150
}

/// M40.1b: `if let <patrón> = <expr> { … } else { … }` — azúcar del parser a un match de dos
/// brazos. Oráculo VM↔intérprete: expresión (con else) y statement (sin else).
#[test]
fn if_let_oracle() {
    // Expresión: `if let Some(v) = o { v } else { def }`.
    oracle_program("\
        fn vo(o: Option<int>, def: int) -> int { if let Option.Some(v) = o { v } else { def } }\n\
        fn main() -> int { vo(Option.Some(42), 0) * 100 + vo(Option.None, 7) }"); // 4207
    // Statement (sin else): solo actúa si el patrón casa.
    oracle_program("\
        fn main() -> int {\n\
        \x20 var s = 0;\n\
        \x20 if let Option.Some(n) = Option.Some(10) { s = s + n; }\n\
        \x20 let none_val: Option<int> = Option.None;\n\
        \x20 if let Option.Some(n) = none_val { s = s + 1000; }\n\
        \x20 s\n\
        }"); // 10
}

/// M40.1c: **patrones de variante anidados** (`Result.Ok(Option.Some(v))`). Exhaustividad
/// conservadora → hace falta un fallback (`Ok(_)`). Oráculo VM↔intérprete (test + codegen).
#[test]
fn patterns_nested_vars_oracle() {
    // Result<Option<int>, string>: cada caso a un dígito.
    oracle_program("\
        fn d(r: Result<Option<int>, string>) -> int {\n\
        \x20 match (r) {\n\
        \x20   Result.Ok(Option.Some(v)) => v,\n\
        \x20   Result.Ok(_) => 100,\n\
        \x20   Result.Err(e) => 200,\n\
        \x20 }\n\
        }\n\
        fn main() -> int {\n\
        \x20 let a: Result<Option<int>, string> = Result.Ok(Option.Some(42));\n\
        \x20 let b: Result<Option<int>, string> = Result.Ok(Option.None);\n\
        \x20 let c: Result<Option<int>, string> = Result.Err(\"x\");\n\
        \x20 d(a) + d(b) + d(c)\n\
        }"); // 42 + 100 + 200 = 342
    // Option<Option<int>> con un segundo nivel de anidamiento.
    oracle_program("\
        fn f(o: Option<Option<int>>) -> int {\n\
        \x20 match (o) { Option.Some(Option.Some(n)) => n, Option.Some(_) => 100, Option.None => 200 }\n\
        }\n\
        fn main() -> int {\n\
        \x20 let x: Option<Option<int>> = Option.Some(Option.Some(7));\n\
        \x20 let z: Option<Option<int>> = Option.None;\n\
        \x20 f(x) + f(z)\n\
        }"); // 7 + 200 = 207
}

/// M40.1d: **patrón de struct** (`Some(Punto { x, y })`). El struct irrefutable cubre la variante
/// sin fallback. Oráculo VM↔intérprete (destructuración + campo con sub-patrón/`_`).
#[test]
fn patterns_struct_oracle() {
    oracle_program("\
        struct Point { x: int, y: int }\n\
        fn f(o: Option<Point>) -> int {\n\
        \x20 match (o) {\n\
        \x20   Option.Some(Point { x, y }) if x > 0 => x + y,\n\
        \x20   Option.Some(Point { x: n, y: _ }) => n,\n\
        \x20   Option.None => 0 - 1,\n\
        \x20 }\n\
        }\n\
        fn main() -> int {\n\
        \x20 let a = Option.Some(Point { x: 3, y: 4 });\n\
        \x20 let b = Option.Some(Point { x: 0 - 9, y: 0 });\n\
        \x20 let c: Option<Point> = Option.None;\n\
        \x20 f(a) * 1000 + (f(b) + 100) * 10 + (f(c) + 10)\n\
        }"); // 7*1000 + (-9+100)*10 + (-1+10) = 7000 + 910 + 9 = 7919
}

/// M40.2: `for x in it` sobre un tipo que implementa `Iterator<T>`. El `for` llama a `next`
/// hasta `None`, ligando el elemento. Oráculo VM↔intérprete (el estado del iterador muta por
/// referencia entre iteraciones).
#[test]
fn iterator_for_oracle() {
    oracle_program("\
        struct Range { actual: int, fin: int }\n\
        impl Iterator<int> for Range {\n\
        \x20 fn next(self) -> Option<int> {\n\
        \x20   if (self.actual < self.fin) {\n\
        \x20     let v = self.actual;\n\
        \x20     self.actual = self.actual + 1;\n\
        \x20     Option.Some(v)\n\
        \x20   } else { Option.None }\n\
        \x20 }\n\
        }\n\
        fn main() -> int {\n\
        \x20 let r = Range { actual: 1, fin: 6 };\n\
        \x20 var sum = 0;\n\
        \x20 for n in r {\n\
        \x20   sum = sum + n * n;\n\
        \x20 }\n\
        \x20 sum\n\
        }"); // 1+4+9+16+25 = 55
}

/// M40.2b: `.iter()` sobre arreglos (iterador genérico `ArrayIter<T>` del prelude) y `range`
/// (iterador `RangeIter`), como iteradores de primera clase. Oráculo VM↔intérprete: el impl
/// genérico de `Iterator` y la sustitución del elemento (`[int].iter()` liga `int`, no `T`).
#[test]
fn iter_range_oracle() {
    oracle_program("\
        fn main() -> int {\n\
        \x20 let xs = [10, 20, 30, 40];\n\
        \x20 var s = 0;\n\
        \x20 for x in xs.iter() { s = s + x; }\n\
        \x20 var p = 1;\n\
        \x20 for i in range(1, 6) { p = p * i; }\n\
        \x20 let it = range(0, 4);\n\
        \x20 var q = 0;\n\
        \x20 for n in it { q = q + n; }\n\
        \x20 s * 10000 + p * 10 + q\n\
        }"); // 100*10000 + 120*10 + 6 = 1001206
}

/// M40.2c: adaptadores PEREZOSOS `.map()`/`.filter()` — métodos genéricos por defecto de
/// `Iterator`, encadenables, respaldados por un closure (`Iter<T>`). Oráculo VM↔intérprete:
/// map cambia de tipo de elemento, filter avanza el origen, y el encadenamiento se evalúa al
/// recorrer. Ejercita métodos genéricos + captura mutable en closures + despacho por receptor.
#[test]
fn lazy_adapters_oracle() {
    oracle_program("\
        fn main() -> int {\n\
        \x20 var a = 0;\n\
        \x20 for x in range(1, 6).map(fn(n: int) -> int { n * n }) { a = a + x; }\n\
        \x20 var b = 0;\n\
        \x20 for x in range(0, 10).filter(fn(n: int) -> bool { n % 2 == 0 }) { b = b + x; }\n\
        \x20 var c = 0;\n\
        \x20 let it = range(1, 11)\n\
        \x20   .map(fn(n: int) -> int { n * 3 })\n\
        \x20   .filter(fn(n: int) -> bool { n > 15 });\n\
        \x20 for x in it { c = c + x; }\n\
        \x20 let xs = [7, 8, 9];\n\
        \x20 var d = 0;\n\
        \x20 for x in xs.iter().filter(fn(n: int) -> bool { n > 7 }) { d = d + x; }\n\
        \x20 a * 1000000 + b * 10000 + c * 100 + d\n\
        }"); // a=55, b=20, c=120, d=17 → 55*1000000 + 20*10000 + 120*100 + 17 = 55200012017... comprobado por el oráculo
}

/// M40.2d: operaciones TERMINALES `.fold()` (reduce a un valor, método genérico sobre el
/// acumulador) y `.collect()` (materializa a `[T]`, puente de vuelta desde la cadena perezosa).
/// Oráculo VM↔intérprete: fold cambia de tipo, collect tras map/filter, y coexistencia con el
/// `fold` EAGER de arreglos (el `[T].fold` cae en la función libre).
#[test]
fn fold_collect_oracle() {
    oracle_program("\
        fn main() -> int {\n\
        \x20 let a = range(1, 6).fold(0, fn(ac: int, x: int) -> int { ac + x });\n\
        \x20 let ys = range(1, 11)\n\
        \x20   .map(fn(n: int) -> int { n * n })\n\
        \x20   .filter(fn(n: int) -> bool { n % 2 == 1 })\n\
        \x20   .collect();\n\
        \x20 let b = ys.fold(0, fn(ac: int, x: int) -> int { ac + x });\n\
        \x20 let zs = [3, 1, 2].iter().map(fn(n: int) -> int { n + 10 }).collect();\n\
        \x20 a * 100000 + b * 100 + ys.len() * 10 + zs[0]\n\
        }"); // a=15, b=165 (1+9+25+49+81), len=5, zs[0]=13
}

/// M40.2e: adaptadores `.take(n)` (perezoso, corta) y `.enumerate()` (pares `(int, T)`), este
/// último consumido con **patrón de tupla en el `for`** (`for (i, x) in it.enumerate()`). Oráculo
/// VM↔intérprete. Ejercita también la inferencia genérica sobre tuplas (`Iter<(int, T)>`).
#[test]
fn take_enumerate_oracle() {
    oracle_program("\
        fn main() -> int {\n\
        \x20 let ys = range(1, 1000).map(fn(n: int) -> int { n * n }).take(4).collect();\n\
        \x20 var a = 0;\n\
        \x20 for x in ys.iter() { a = a + x; }\n\
        \x20 var b = 0;\n\
        \x20 for par in [10, 20, 30].iter().enumerate() { let (i, v) = par; b = b + i * v; }\n\
        \x20 var c = 0;\n\
        \x20 for (i, v) in range(5, 100).enumerate().take(3) { c = c + i * 100 + v; }\n\
        \x20 a * 10000 + b * 100 + c\n\
        }"); // a=1+4+9+16=30, b=0*10+1*20+2*30=80, c=(0*100+5)+(1*100+6)+(2*100+7)=5+106+207=318
}

/// M40.2f: `.skip(n)` (descarta los primeros n), `.zip(otra)` (pares `(T, U)`, se agota con el
/// más corto; método genérico) y `.sum()` (terminal, función libre sobre `Iter<int>` vía UFCS).
/// Oráculo VM↔intérprete: zip con tipos distintos + patrón de tupla, y sum encadenado.
#[test]
fn skip_zip_sum_oracle() {
    oracle_program("\
        fn main() -> int {\n\
        \x20 let a = sum(range(0, 100).skip(5).take(3));\n\
        \x20 var b = 0;\n\
        \x20 for (n, c) in range(1, 50).zip([\"a\", \"bb\", \"ccc\"].iter()) { b = b + n * c.len(); }\n\
        \x20 let d = range(1, 6).map(fn(n: int) -> int { n * n }).sum();\n\
        \x20 a * 100000 + b * 100 + d\n\
        }"); // a=5+6+7=18, b=1*1+2*2+3*3=14, d=55 → 18*100000+14*100+55
}

/// M40.3a: `@derive(Hash)` sobre struct y enum, más `char_code` y las impls de Hash de
/// primitivos (int/bool/char/string) del prelude. El hash se calcula EN raylang (recursión por
/// `.hash()` de campos), así que el oráculo VM↔intérprete verifica que ambos motores producen el
/// MISMO entero. Cubre el fix de colisión de posiciones (dos derivados con campos de tipos
/// distintos que van a `int#hash` vs `string#hash`).
#[test]
fn hash_derive_oracle() {
    oracle_program("\
        @derive(Hash, Eq)\n\
        struct Point { x: int, y: int }\n\
        @derive(Hash)\n\
        struct Persona { name: string, age: int }\n\
        @derive(Hash)\n\
        enum Color { Rojo, Verde, RGB(int, int, int) }\n\
        fn main() -> int {\n\
        \x20 let p = Point { x: 3, y: 4 };\n\
        \x20 let a = Persona { name: \"Ada\", age: 36 };\n\
        \x20 let same = if (p.hash() == (Point { x: 3, y: 4 }).hash()) { 1 } else { 0 };\n\
        \x20 p.hash() + a.hash() * 7 + Color.RGB(1, 2, 3).hash() * 13 + char_code('Z') + same * 100000\n\
        }");
}

/// Ejecuta un programa en la VM con el GC en **modo estrés** (recolecta en cada
/// punto seguro) y exige que el resultado coincida con el intérprete. Es la
/// prueba clave del GC: si una raíz faltara, un valor vivo se liberaría y el
/// resultado cambiaría o reventaría.
fn oracle_stress(src: &str) {
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    crate::checker::check(&mut prog).expect("check ok");
    let interp = crate::interpreter::run(&prog).expect("intérprete ok");
    let compiled = compile_program(&prog).expect("compila");
    let mut vm = Vm::new(&compiled);
    vm.cur.heap.stress = true;
    let result = vm.run().expect("vm ok");
    let vm_result = to_value(&vm.cur.heap, &compiled.structs, &compiled.enums, &result);
    assert_eq!(interp, vm_result, "VM (estrés) y intérprete difieren en:\n{}", src);
}

// ----- M2.1 / M2.2: expresiones -----

#[test]
fn arithmetic_coincide_con_el_interpreter() {
    oracle_int("1 + 2 * 3");
    oracle_int("(1 + 2) * 3");
    oracle_int("10 - 2 - 3");
    oracle_int("17 % 5");
    oracle_int("-5 + 3");
    oracle_int("2 * 3 * 4 - 10 / 2");
}

#[test]
fn comparisons_and_bools() {
    assert_eq!(run_vm("3 < 5"), Value::Bool(true));
    assert_eq!(run_vm("3 == 5"), Value::Bool(false));
    assert_eq!(run_vm("!(2 > 1)"), Value::Bool(false));
    assert_eq!(run_vm("true"), Value::Bool(true));
}

#[test]
fn flotantes() {
    assert_eq!(run_vm("1.0 / 2.0"), Value::Float(0.5));
    assert_eq!(run_vm("1.5 + 1.5"), Value::Float(3.0));
}

#[test]
fn division_by_zero_is_error() {
    let chunk = compile_expr(&expr_of("10 / 0")).unwrap();
    assert!(run(&chunk).unwrap_err().msg.contains("division"));
}

#[test]
fn if_as_expression_matches_interpreter() {
    oracle_int("if (3 < 5) { 10 } else { 20 }");
    oracle_int("if (3 > 5) { 10 } else { 20 }");
    oracle_int("if (1 < 2) { if (2 < 3) { 1 } else { 2 } } else { 3 }");
    oracle_int("if (1 < 2 && 3 < 4) { 7 } else { 8 }");
}

#[test]
fn if_without_else_is_unit() {
    assert_eq!(run_vm("if (true) { }"), Value::Unit);
    assert_eq!(run_vm("if (false) { }"), Value::Unit);
}

#[test]
fn logical_operators_short_circuit() {
    assert_eq!(run_vm("true && true"), Value::Bool(true));
    assert_eq!(run_vm("true && false"), Value::Bool(false));
    assert_eq!(run_vm("false || true"), Value::Bool(true));
    assert_eq!(run_vm("false && (1 / 0 == 0)"), Value::Bool(false));
    assert_eq!(run_vm("true || (1 / 0 == 0)"), Value::Bool(true));
}

#[test]
fn block_with_statements_and_final_value() {
    assert_eq!(run_vm("{ 1; 2; 3 }"), Value::Int(3));
    assert_eq!(run_vm("{ 1; }"), Value::Unit);
}

// ----- M2.3: programas completos (variables, while, llamadas) -----

#[test]
fn recursion_fibonacci() {
    oracle_program(
        "fn fib(n: int) -> int { if (n < 2) { n } else { fib(n - 1) + fib(n - 2) } }
         fn main() -> int { fib(10) }",
    );
}

#[test]
fn factorial_con_while_y_mutation() {
    oracle_program(
        "fn main() -> int {
            var n: int = 5; var f: int = 1;
            while (n > 1) { f = f * n; n = n - 1; }
            f
         }",
    );
}

#[test]
fn return_val_early() {
    oracle_program(
        "fn sign(x: int) -> int { if (x < 0) { return -1; } if (x > 0) { return 1; } 0 }
         fn main() -> int { sign(-7) + sign(0) + sign(42) }",
    );
}

#[test]
fn gcd_recursive() {
    oracle_program(
        "fn gcd(a: int, b: int) -> int { if (b == 0) { a } else { gcd(b, a % b) } }
         fn main() -> int { gcd(1071, 462) }",
    );
}

/// M13.3a: recursión infinita → ambos motores cortan con el MISMO error de
/// desbordamiento, en vez de colgarse o reventar la pila. Es el oráculo del
/// límite compartido (`MAX_CALL_DEPTH` == `MAX_FRAMES`). Corre dentro del hilo de
/// pila grande para que el intérprete alcance el tope sin desbordar la pila del
/// hilo de test (que es pequeña por defecto). **La recursión es NO de cola**
/// (`1 + bucle(...)`): la de cola, con el TCO de M13.3b, sería un bucle infinito
/// legítimo (O(1) marcos) y nunca desbordaría —ese es justo el punto del TCO—.
#[test]
fn arithmetic_overflow_oracle() {
    // M34 (SPEC §8): el desbordamiento de int es ERROR de ejecución idéntico en ambos
    // motores (antes: panic en debug / wrap silencioso en release — dependía del build).
    let cases = [
        "fn main() -> int { let m = 9223372036854775807; m + 1 }",       // Add
        "fn main() -> int { let m = -9223372036854775807 - 1; m - 1 }",  // Sub
        "fn main() -> int { let m = 9223372036854775807; m * 2 }",       // Mul
        "fn main() -> int { let m = -9223372036854775807 - 1; m / -1 }", // Div (MIN/-1)
        "fn main() -> int { let m = -9223372036854775807 - 1; m % -1 }", // Rem (MIN%-1)
        "fn main() -> int { let m = -9223372036854775807 - 1; -m }",     // Neg (-MIN)
    ];
    for src in cases {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        let interp = crate::interpreter::run(&prog).expect_err("el intérprete must errar");
        let compiled = compile_program(&prog).expect("compila");
        let vm = run_program(&compiled).expect_err("la VM must errar");
        assert!(interp.msg.contains("arithmetic overflow on int"), "interp: {} ({src})", interp.msg);
        assert_eq!(interp.msg, vm.msg, "ambos engines idénticos ({src})");
    }
    // Y la aritmética al borde SIN desbordar sigue funcionando igual en ambos.
    let src = "fn main() -> int { let m = 9223372036854775806; print(m + 1); 0 }";
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    crate::checker::check(&mut prog).expect("check ok");
    crate::interpreter::run(&prog).expect("interp ok");
    let compiled = compile_program(&prog).expect("compila");
    run_program(&compiled).expect("vm ok");
}

/// M42.1: **fuel** — límite de instrucciones de la VM. Un bucle infinito aborta con fuel finito
/// (no cuelga); un programa que termina dentro del presupuesto da su resultado normal.
#[test]
fn fuel_limits_execution() {
    fn compile(src: &str) -> CompiledProgram {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        compile_program(&prog).expect("compila")
    }
    // Bucle infinito: con fuel finito, aborta (sin fuel colgaría, así que no se prueba sin límite).
    let inf = compile("fn main() -> int { var i = 0; while (true) { i = i + 1; } 0 }");
    let err = run_program_with_limit(&inf, Some(50_000), None).expect_err("must agotar el fuel");
    assert!(err.msg.contains("fuel"), "mensaje de fuel: {}", err.msg);
    // Un programa que termina dentro del presupuesto da el mismo resultado que sin límite.
    let ok = compile("fn main() -> int { var s = 0; var i = 0; while (i < 100) { s = s + i; i = i + 1; } s }");
    assert_eq!(run_program_with_limit(&ok, Some(1_000_000), None).unwrap(), Value::Int(4950));
    assert_eq!(run_program_with_limit(&ok, None, None).unwrap(), Value::Int(4950)); // None = sin límite
}

/// M38.1a: `transfer_value` re-aloja un subgrafo de un heap a otro con handles del destino.
/// Cubre lo estructural (arreglo con struct + string), el **sharing interno** (un objeto alcanzado
/// por dos caminos se copia UNA vez) y los **ciclos** (que un deep-copy ingenuo colgaría).
#[test]
fn transfer_value_between_heaps() {
    use std::collections::HashMap;
    // (1) Estructural: [1, P{x:2}, "hi"] → estructuralmente igual, con handles del destino.
    {
        let mut a = Heap::new();
        let p = a.allocate(Obj::Struct(VmStruct { struct_idx: 0, fields: vec![HeapValue::Int(2)] }));
        let top = a.allocate(Obj::Array(vec![HeapValue::Int(1), HeapValue::Obj(p), HeapValue::Str("hi".into())]));
        let mut b = Heap::new();
        let mut remap = HashMap::new();
        let tv = transfer_value(&a, &mut b, &HeapValue::Obj(top), &mut remap);
        assert_eq!(to_value(&b, &[crate::bytecode::CompiledStruct { name: "P".into(), fields: vec!["x".into()] }], &[], &tv), to_value(&a, &[crate::bytecode::CompiledStruct { name: "P".into(), fields: vec!["x".into()] }], &[], &HeapValue::Obj(top)), "estructuralmente iguales");
        assert_eq!(b.live(), 2, "se copiaron 2 objetos (array + struct)");
    }
    // (2) Sharing: [sub, sub] con el MISMO handle → tras transferir, ambos apuntan al mismo destino.
    {
        let mut a = Heap::new();
        let sub = a.allocate(Obj::Array(vec![HeapValue::Int(7)]));
        let top = a.allocate(Obj::Array(vec![HeapValue::Obj(sub), HeapValue::Obj(sub)]));
        let mut b = Heap::new();
        let mut remap = HashMap::new();
        let tv = transfer_value(&a, &mut b, &HeapValue::Obj(top), &mut remap);
        let nt = tv.handle().unwrap();
        let (h0, h1) = match b.get(nt) {
            Obj::Array(e) => (e[0].handle().unwrap(), e[1].handle().unwrap()),
            _ => panic!("esperaba array"),
        };
        assert_eq!(h0, h1, "el sharing internal se preserves (un solo objeto copiado)");
        assert_eq!(b.live(), 2, "sharing → 2 objetos (top + sub), no 3");
    }
    // (3) Ciclo: arr -> cell -> arr. Debe terminar y preservar el ciclo.
    {
        let mut a = Heap::new();
        let arr = a.allocate(Obj::Array(Vec::new())); // placeholder
        let cell = a.allocate(Obj::Cell(HeapValue::Obj(arr)));
        *a.get_mut(arr) = Obj::Array(vec![HeapValue::Obj(cell)]); // cierra el ciclo
        let mut b = Heap::new();
        let mut remap = HashMap::new();
        let tv = transfer_value(&a, &mut b, &HeapValue::Obj(arr), &mut remap);
        let narr = tv.handle().unwrap();
        let ncell = match b.get(narr) { Obj::Array(e) => e[0].handle().unwrap(), _ => panic!() };
        let back = match b.get(ncell) { Obj::Cell(HeapValue::Obj(h)) => *h, _ => panic!() };
        assert_eq!(back, narr, "el ciclo se preserves (la celda apunta de vuelta al array)");
        assert_eq!(b.live(), 2, "ciclo → 2 objetos (array + celda)");
    }
}

/// M42.2: **tope de heap** — límite de objetos vivos de la VM. Un programa que retiene un montón
/// de objetos (aquí, un arreglo que crece sin cesar) aborta al rebasar el tope; uno frugal, no.
#[test]
fn heap_cap_limits_live_objects() {
    fn compile(src: &str) -> CompiledProgram {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        compile_program(&prog).expect("compila")
    }
    // Retiene objetos vivos sin parar (cada iteración empuja un arreglo nuevo a `xs`, que sigue
    // alcanzable). Con un tope bajo, el GC no puede liberarlos → aborta.
    let crece = compile(
        "fn main() -> int { var xs: [[int]] = []; var i = 0; while (i < 100000) { xs.push([i]); i = i + 1; } 0 }",
    );
    let err = run_program_with_limit(&crece, None, Some(1_000)).expect_err("must rebasar el tope");
    assert!(err.msg.contains("heap cap"), "mensaje de tope: {}", err.msg);
    // Un programa frugal (no retiene) termina normal aun con tope bajo: el GC recicla la basura.
    let frugal = compile("fn main() -> int { var s = 0; var i = 0; while (i < 10000) { s = s + i; i = i + 1; } s }");
    assert_eq!(run_program_with_limit(&frugal, None, Some(1_000)).unwrap(), Value::Int(49995000));
}

#[test]
    fn overflow_recursion_oracle() {
    let (interp_msg, vm_msg) = crate::with_big_stack(|| {
        let src = "fn loop(n: int) -> int { 1 + loop(n + 1) }
                   fn main() -> int { loop(0) }";
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        let interp = crate::interpreter::run(&prog).expect_err("el intérprete must errar");
        let compiled = compile_program(&prog).expect("compila");
        let vm = run_program(&compiled).expect_err("la VM must errar");
        (interp.msg, vm.msg)
    });
    assert!(interp_msg.contains("stack overflow"), "intérprete: {interp_msg}");
    assert!(vm_msg.contains("stack overflow"), "vm: {vm_msg}");
    // Ambos motores reportan exactamente el mismo mensaje.
    assert_eq!(interp_msg, vm_msg, "los dos engines difieren en el mensaje");
}

/// M13.2a: aserciones que pasan no alteran el resultado (oráculo normal).
#[test]
fn assert_passes_oracle() {
    oracle_program(
        "fn main() -> int {
            assert(1 + 1 == 2);
            assert_eq(2 * 3, 6);
            assert_eq(\"ab\", \"a\" + \"b\");
            42
         }",
    );
}

/// M61.3: ergonomía de Option/Result (traits `OptionOps`/`ResultOps` del prelude) + Eq/Show
/// de bytes y arreglos. Los MISMOS nombres (`unwrap_or`/`expect`/`unwrap`) existen para
/// Option y Result: el despacho por punto resuelve por el tipo del receptor.
#[test]
fn option_result_ergonomics_oracle() {
    oracle_program(
        "fn half(x: int) -> Option<int> {\n\
         \x20 if (x % 2 == 0) { Option.Some(x / 2) } else { Option.None }\n\
         }\n\
         fn main() -> int {\n\
         \x20 var n = 0;\n\
         \x20 if (half(8).is_some() && half(3).is_none()) { n = n + 1; }\n\
         \x20 n = n + half(8).unwrap_or(0);\n\
         \x20 n = n + half(3).unwrap_or(100);\n\
         \x20 n = n + half(8).map(fn(x: int) -> int { x * 10 }).unwrap_or(0);\n\
         \x20 n = n + half(8).expect(\"par\");\n\
         \x20 let r: Result<int, string> = half(8).ok_or(\"impar\");\n\
         \x20 if (r.is_ok()) { n = n + r.unwrap(); }\n\
         \x20 let e: Result<int, string> = half(3).ok_or(\"impar\");\n\
         \x20 if (e.is_err() && e.ok().is_none()) { n = n + e.unwrap_or(1000); }\n\
         \x20 if (b\"ab\".eq(b\"ab\") && [1, 2].eq([1, 2]) && !([1].eq([2]))) { n = n + 1; }\n\
         \x20 if ([1, 2].show() == \"[1, 2]\" && b\"ab\".show() == \"6162\") { n = n + 1; }\n\
         \x20 if (sum_float([1.5, 2.5].iter()) == 4.0) { n = n + 1; }\n\
         \x20 n\n\
         }",
    );
}

/// M62.1: terminales `any`/`all`/`count` (lazy, en el trait) + `any`/`all` eager sobre
/// arreglos (bucles directos; mismo nombre, despacho por receptor). Cortocircuito incluido.
#[test]
fn any_all_count_oracle() {
    oracle_program(
        "fn main() -> int {\n\
         \x20 var n = 0;\n\
         \x20 if ([1, 2, 3].any(fn(x: int) -> bool { x == 2 })) { n = n + 1; }\n\
         \x20 if ([1, 2, 3].all(fn(x: int) -> bool { x > 0 })) { n = n + 1; }\n\
         \x20 if (!([1, 2, 3].all(fn(x: int) -> bool { x < 3 }))) { n = n + 1; }\n\
         \x20 let empty: [int] = [];\n\
         \x20 if (!empty.any(fn(x: int) -> bool { true }) && empty.all(fn(x: int) -> bool { false })) { n = n + 1; }\n\
         \x20 if (range(1, 1000000).map(fn(x: int) -> int { x * 2 }).any(fn(x: int) -> bool { x > 10 })) { n = n + 1; }\n\
         \x20 n = n + range(0, 50).filter(fn(x: int) -> bool { x % 10 == 0 }).count();\n\
         \x20 if ([9, 9].iter().all(fn(x: int) -> bool { x == 9 })) { n = n + 1; }\n\
         \x20 n\n\
         }",
    );
}

/// M90.5: terminal `find` (corta en el primero), adaptador `chain` (secuencia, compone
/// con map), terminales `min`/`max` (genéricos `T: Ord`; None sobre vacío) y `Ord` para
/// bool (false < true) y bytes (lexicográfico) vía `sort`/`less`.
#[test]
fn find_chain_min_max_oracle() {
    oracle_program(
        "fn main() -> int {\n\
         \x20 var n = 0;\n\
         \x20 match (range(1, 1000000).find(fn(x: int) -> bool { x * x > 10 })) {\n\
         \x20   Option.Some(v) => { n = n + v; },\n\
         \x20   Option.None => { },\n\
         \x20 }\n\
         \x20 match ([1, 2].iter().find(fn(x: int) -> bool { x > 5 })) {\n\
         \x20   Option.Some(v) => { n = n + v; },\n\
         \x20   Option.None => { n = n + 1000; },\n\
         \x20 }\n\
         \x20 n = n + sum(range(1, 4).chain([10, 20].iter()));\n\
         \x20 n = n + range(0, 3).chain(range(10, 12)).map(fn(x: int) -> int { x * 2 }).count();\n\
         \x20 match (min([3, 1, 2].iter())) {\n\
         \x20   Option.Some(m) => { n = n + m * 10; },\n\
         \x20   Option.None => { },\n\
         \x20 }\n\
         \x20 match ([5, 9, 7].iter().max()) {\n\
         \x20   Option.Some(m) => { n = n + m * 100; },\n\
         \x20   Option.None => { },\n\
         \x20 }\n\
         \x20 let empty: [int] = [];\n\
         \x20 match (min(empty.iter())) {\n\
         \x20   Option.Some(m) => { n = n + m; },\n\
         \x20   Option.None => { n = n + 10000; },\n\
         \x20 }\n\
         \x20 let bs = sort([true, false, true]);\n\
         \x20 if (!bs[0] && bs[2]) { n = n + 100000; }\n\
         \x20 if (b\"ab\".less(b\"abc\") && b\"abc\".less(b\"abd\") && !b\"b\".less(b\"abc\")) { n = n + 1000000; }\n\
         \x20 n\n\
         }",
    ); // 4 + 1000 + 36 + 5 + 10 + 900 + 10000 + 100000 + 1000000
}

/// M13.2a: `panic` / `assert_eq` que falla → ambos motores cortan con el MISMO mensaje.
#[test]
fn panic_y_assert_fails_oracle() {
    for (src, expected) in [
        ("fn main() -> int { panic(\"boom\"); 0 }", "boom"),
        ("fn main() -> int { assert_eq(2 + 2, 5); 0 }", "assert_eq failed: 4 != 5"),
        ("fn main() -> int { assert(false); 0 }", "assertion failed"),
    ] {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        let interp = crate::interpreter::run(&prog).expect_err("el intérprete must errar");
        let compiled = compile_program(&prog).expect("compila");
        let vm = run_program(&compiled).expect_err("la VM must errar");
        assert_eq!(interp.msg, expected, "intérprete: {}", src);
        assert_eq!(vm.msg, expected, "vm: {}", src);
    }
}

/// Opt.12: el plegado de constantes. (a) Una expresión de literales se compila a
/// UNA constante (cero opcodes aritméticos en el chunk) con el valor correcto.
/// (b) Lo que puede trapear (división por cero, overflow) NO se pliega: sigue
/// dando el MISMO error de runtime en ambos motores, con su posición.
#[test]
fn constant_folding() {
    let src = "fn main() -> int { 1 + 2 * 3 - 4 / 2 }";
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    crate::checker::check(&mut prog).expect("check ok");
    let compiled = compile_program(&prog).expect("compila");
    let main = compiled.functions.iter().find(|f| f.name == "main").expect("main");
    let arith = main
        .chunk
        .code
        .iter()
        .filter(|op| matches!(op, OpCode::Add | OpCode::Sub | OpCode::Mul | OpCode::Div))
        .count();
    assert_eq!(arith, 0, "la expresión de literals must plegarse: {:?}", main.chunk.code);
    assert_eq!(run_program(&compiled).expect("runs"), Value::Int(5));

    // Lo trapeante queda para el runtime, idéntico en ambos motores.
    for (src, msg) in [
        ("fn main() -> int { 1 / 0 }", "integer division by zero"),
        ("fn main() -> int { 7 % 0 }", "modulo by zero"),
        ("fn main() -> int { 9223372036854775807 + 1 }", "arithmetic overflow on int"),
    ] {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        let interp = crate::interpreter::run(&prog).expect_err("el intérprete must errar");
        let compiled = compile_program(&prog).expect("compila");
        let vm = run_program(&compiled).expect_err("la VM must errar");
        assert_eq!(interp.msg, msg, "intérprete: {}", src);
        assert_eq!(vm.msg, msg, "vm: {}", src);
    }

    // Floats y bools también se pliegan, con el resultado del oráculo.
    let src = "fn main() -> int { if (0.5 * 4.0 == 2.0 && !(1 > 2)) { 42 } else { 0 } }";
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    crate::checker::check(&mut prog).expect("check ok");
    let interp = crate::interpreter::run(&prog).expect("interp runs");
    let compiled = compile_program(&prog).expect("compila");
    assert_eq!(run_program(&compiled).expect("vm runs"), interp);
    assert_eq!(interp, Value::Int(42));
}

/// M79: la traza de llamadas de un error de runtime debe ser IDÉNTICA (nombres +
/// posiciones) entre ambos motores: panic anidado (con marcos repetidos por
/// recursión no-cola), assert del prelude (la traza cruza al fuente inyectado),
/// división por cero en un helper, error dentro de una closure, y llamada en cola
/// (el marco reutilizado aparece UNA vez, con el nombre final).
#[test]
fn stack_trace_oracle() {
    for src in [
        // panic anidado con recursión NO-cola (el `+ 0` evita el TCO): main → middle → boom×3
        "fn boom(n: int) -> int { if (n == 0) { panic(\"boom\"); } boom(n - 1) + 0 }\n\
         fn middle() -> int { boom(2) }\n\
         fn main() -> int { middle() }",
        // assert del prelude: la traza atraviesa `assert` (fuente inyectado)
        "fn main() -> int { assert(false); 0 }",
        // división por cero en un helper
        "fn div(a: int, b: int) -> int { a / b }\nfn main() -> int { div(1, 0) }",
        // error dentro de una función anónima (el nombre `<fn#0>` debe casar)
        "fn applies(f: fn(int) -> int, x: int) -> int { f(x) }\n\
         fn main() -> int { applies(fn(n: int) -> int { n / 0 }, 3) }",
        // llamada en cola: el marco se reutiliza (la traza NO crece con la recursión)
        "fn account(n: int) -> int { if (n == 0) { panic(\"fin\"); } account(n - 1) }\n\
         fn main() -> int { account(5) }",
    ] {
        let tokens = crate::lexer::lex(src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        let interp = crate::interpreter::run(&prog).expect_err("el intérprete must errar");
        let compiled = compile_program(&prog).expect("compila");
        let vm = run_program(&compiled).expect_err("la VM must errar");
        assert!(!vm.trace.is_empty(), "la VM must adjuntar traza: {}", src);
        assert_eq!(interp.trace, vm.trace, "trazas distintas en:\n{}", src);
        // La cabecera y la entrada 0 cuentan lo mismo (nombre del marco interno aparte).
        assert_eq!((vm.trace[0].line, vm.trace[0].col), (vm.line, vm.col), "entry 0 = posición del error: {}", src);
    }

    // Forma de la traza: los nombres, de dentro afuera. Los `+ 0` hacen NO-cola
    // cada llamada (sin ellos, el TCO reutiliza los marcos de main/middle y la
    // traza sería [boom, boom, boom] — verificado por el oráculo de arriba).
    let src = "fn boom(n: int) -> int { if (n == 0) { panic(\"boom\"); } boom(n - 1) + 0 }\n\
               fn middle() -> int { boom(2) + 0 }\n\
               fn main() -> int { middle() + 0 }";
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    crate::checker::check(&mut prog).expect("check ok");
    let compiled = compile_program(&prog).expect("compila");
    let vm = run_program(&compiled).expect_err("la VM must errar");
    let names: Vec<&str> = vm.trace.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["boom", "boom", "boom", "middle", "main"]);

    // Y con TCO el marco reutilizado aparece UNA vez: `main → cuenta` y toda la
    // recursión de `cuenta` son colas → UN solo marco, renombrado a `cuenta`.
    let src = "fn account(n: int) -> int { if (n == 0) { panic(\"fin\"); } account(n - 1) }\n\
               fn main() -> int { account(5) }";
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    crate::checker::check(&mut prog).expect("check ok");
    let compiled = compile_program(&prog).expect("compila");
    let vm = run_program(&compiled).expect_err("la VM must errar");
    let names: Vec<&str> = vm.trace.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["account"], "TCO: el marco en cola se reutiliza");
}

/// M15.1a: la stdlib de matemáticas en el oráculo. Las funciones float se enrutan a `int` por la
/// comparación de floats del propio lenguaje (NaN != NaN impediría comparar `Value::Float`
/// directamente); abs/min/max sobre `int` devuelven `int`. El último caso fija la semántica de
/// **borde** de `f64`: `sqrt(-1.0)` da `NaN` en ambos motores → `NaN == NaN` es `false` → `0`.
#[test]
fn math_oracle() {
    // M49: abs/min/max/pi/e se movieron a `std/math` (funciones puras en raylang; las cubre la
    // integración `math_cli`, en ambos motores). El ORÁCULO prueba solo los primitivos internos `__x`
    // —el que computa, aún builtin, sin necesidad del loader—; el envoltorio `math.sqrt` lo cierra el
    // ejemplo `matematicas.ray`. Verificados por igualdad (ambos motores calculan idéntico).
    oracle_int("if (__sqrt(16.0) == 4.0) { 1 } else { 0 }");
    oracle_int("if (__pow(2.0, 10.0) == 1024.0) { 1 } else { 0 }");
    oracle_int("if (__floor(3.7) == 3.0) { 1 } else { 0 }");
    oracle_int("if (__ceil(3.2) == 4.0) { 1 } else { 0 }");
    oracle_int("if (__round(2.5) == 3.0) { 1 } else { 0 }");
    oracle_int("if (__sin(0.0) == 0.0) { 1 } else { 0 }");
    oracle_int("if (__cos(0.0) == 1.0) { 1 } else { 0 }");
    oracle_int("if (__ln(1.0) == 0.0) { 1 } else { 0 }");
    oracle_int("if (__log10(1000.0) == 3.0) { 1 } else { 0 }");
    oracle_int("if (__exp(0.0) == 1.0) { 1 } else { 0 }");
    // Borde: NaN se comporta igual en ambos motores (NaN != NaN → la rama else).
    oracle_int("if (__sqrt(0.0 - 1.0) == __sqrt(0.0 - 1.0)) { 1 } else { 0 }");
    // M65.2: trig inversa y compañía (valores exactos en f64 → la igualdad vale).
    oracle_int("if (__asin(1.0) == __acos(0.0)) { 1 } else { 0 }"); // ambas = π/2 exacto
    oracle_int("if (__asin(0.0) == 0.0) { 1 } else { 0 }");
    oracle_int("if (__atan(0.0) == 0.0) { 1 } else { 0 }");
    oracle_int("if (__atan2(0.0, 1.0) == 0.0) { 1 } else { 0 }");
    oracle_int("if (__atan2(1.0, 0.0) == __asin(1.0)) { 1 } else { 0 }"); // π/2 por ambos caminos
    oracle_int("if (__atan2(0.0, 0.0 - 1.0) > 3.14) { 1 } else { 0 }"); // π (rama x<0)
    oracle_int("if (__log2(1024.0) == 10.0) { 1 } else { 0 }");
    oracle_int("if (__trunc(3.7) == 3.0) { 1 } else { 0 }");
    oracle_int("if (__trunc(0.0 - 3.7) == 0.0 - 3.0) { 1 } else { 0 }"); // hacia cero, no floor
    // Fuera de dominio → NaN, igual en ambos motores.
    oracle_int("if (__asin(2.0) == __asin(2.0)) { 1 } else { 0 }");
}

/// M27.1: tuplas — retorno múltiple, acceso `.N`, desestructuración (`_`), heterogéneas. Erasure a
/// arreglos → ambos motores coinciden.
#[test]
fn tuples_oracle() {
    oracle_program("fn dm(a: int, b: int) -> (int, int) { (a / b, a % b) } fn main() -> int { let t = dm(17, 5); t.0 + t.1 * 10 }"); // 3 + 20 = 23
    oracle_program("fn main() -> int { let (q, r) = (7, 3); q * r }"); // 21
    oracle_program("fn main() -> int { let (_, b, _) = (1, 42, 9); b }"); // 42 (descarta con _)
    oracle_program("fn main() -> int { let t = (\"x\", 5, true); if (t.2) { t.1 } else { 0 } }"); // 5 (heterogénea)
    oracle_program("fn swap(a: int, b: int) -> (int, int) { (b, a) } fn main() -> int { let (x, y) = swap(1, 2); x * 10 + y }"); // 21
    // Tupla anidada (el acceso encadenado `t.0.1` choca con el float `0.1` en el lexer → binding
    // intermedio; limitación documentada de M27.1).
    oracle_program("fn main() -> int { let t = ((1, 2), 3); let inner = t.0; inner.1 + t.1 }"); // 2 + 3 = 5
}

/// M27.2: bucle `for` — rango, arreglo, string, Map `(k, v)`, `_`. Ambos motores coinciden.
/// M27.5: constantes de nivel superior. Resueltas como `Ident` globales → ambos motores coinciden.
#[test]
fn const_oracle() {
    oracle_program("const MAX: int = 100; fn main() -> int { MAX - 42 }"); // 58
    oracle_program("const A: int = 7; const B: int = 3; fn main() -> int { A * B }"); // 21
    oracle_program("const NEG: int = -5; fn main() -> int { NEG + 10 }"); // 5
    oracle_program("const PI: float = 3.0; fn f(r: float) -> float { PI * r } fn main() -> int { f(4.0) as int }"); // 12
    oracle_program("const ON: bool = true; fn main() -> int { if (ON) { 1 } else { 0 } }"); // 1
    oracle_int("if (\"x\" == \"x\") { 1 } else { 0 }"); // control
}

/// M27.4: casts `as` — int↔float, char↔int. Cambian la representación → ambos motores coinciden.
#[test]
fn cast_oracle() {
    oracle_int("(3.99 as int) + (2.1 as int)");        // 3 + 2 = 5
    oracle_int("('A' as int) + ('a' as int)");         // 65 + 97 = 162
    oracle_int("if ((7 as float) == 7.0) { 1 } else { 0 }"); // 1
    oracle_int("if ((66 as char) == 'B') { 1 } else { 0 }"); // 1
    oracle_int("(0.0 - 4.7) as int");                  // -4 (trunca hacia cero)
    oracle_program("fn main() -> int { let s = 10; let n = 4; let avg = (s as float) / (n as float); avg as int }"); // 2
}

/// M27.3: interpolación de strings `"...${expr}..."`. Desazucara a `+ to_string(...)` → ambos
/// motores coinciden. Se enruta a `int` comparando la longitud (print diferido en `oracle_int`).
#[test]
fn interpolation_oracle() {
    oracle_int("\"x=${1}\".len()");                     // "x=1" → 3
    oracle_int("\"${1}+${2}=${3}\".len()");             // "1+2=3" → 5
    oracle_int("if (\"a${1}b\" == \"a1b\") { 1 } else { 0 }");   // 1
    oracle_int("if (\"${2 + 3}\" == \"5\") { 1 } else { 0 }");   // 1
    oracle_int("if (\"${true}/${'z'}\" == \"true/z\") { 1 } else { 0 }"); // 1
    // Las llaves son SIEMPRE literales (sin `{{`/`}}`); solo `${` es especial.
    oracle_int("\"llave {lit}\".len()"); // "llave {lit}" = 11
    // Un `$` que no precede a `{` es literal (sin escape): "$5" → 2 caracteres.
    oracle_int("\"$5\".len()");                         // 2
    // `\$` escapa un `${` literal: "\${x}" → "${x}" = 4 caracteres, sin interpolar.
    oracle_int("\"\\${x}\".len()");                     // 4 (literal "${x}")
    // Interpolación con una variable local.
    oracle_program("fn main() -> int { let n = 42; if (\"n=${n}\" == \"n=42\") { 1 } else { 0 } }");
}

/// M28.3: enteros sin signo con tamaño (u8/u32/u64). Aritmética con wrapping dentro del ancho,
/// bitops, comparación sin signo, conversión con `as`. Ambos motores comparten la máscara → iguales.
#[test]
fn uint_oracle() {
    oracle_int("((200 as u8) + (100 as u8)) as int");   // 300 mod 256 = 44
    oracle_int("(511 as u8) as int");                    // 255 (enmascarado)
    oracle_int("((4294967295 as u32) + (1 as u32)) as int"); // wrap a 0
    oracle_int("((1 as u32) << (8 as u32)) as int");     // 256
    oracle_int("(~(0 as u8)) as int");                   // 255
    oracle_int("((250 as u8) - (5 as u8)) as int");      // 245
    oracle_int("((0 as u8) - (1 as u8)) as int");        // wrap a 255
    oracle_int("if ((255 as u8) > (1 as u8)) { 1 } else { 0 }"); // 1 (sin signo)
    oracle_int("(((240 as u8) & (15 as u8)) | (1 as u8)) as int"); // (0) | 1 = 1
    oracle_int("(((1000000 as u64) * (1000000 as u64))) as int"); // 10^12 (cabe en u64, no en u32)
    // Round-trip de anchos.
    oracle_int("(((300 as u8) as u32) as int)");         // 300&0xFF=44
    oracle_program("fn dobla(x: u32) -> u32 { x + x } fn main() -> int { dobla(10 as u32) as int }"); // 20
}

/// M28.3b: coerción de literal entero polimórfico — un literal adopta el ancho uint del contexto
/// (tipo esperado u operando). Baja a un `as` → ambos motores coinciden.
#[test]
fn uint_literal_oracle() {
    oracle_program("fn main() -> int { let x: u8 = 5; x as int }");            // 5
    oracle_program("fn main() -> int { let x: u8 = 200; let y: u8 = x + 100; y as int }"); // 44
    oracle_program("fn main() -> int { let z: u8 = 200 + 100; z as int }");    // 44 (ambos literales)
    oracle_program("fn main() -> int { let b: u32 = 4000000000; b as int }");  // 4000000000
    oracle_program("fn main() -> int { let a: [u8] = [1, 2, 3]; a[2] as int }"); // 3
    oracle_program("fn f(x: u8) -> u8 { x } fn main() -> int { f(42) as int }"); // 42 (arg literal)
    oracle_program("fn main() -> int { let m: u32 = (1 << 8) + 1; m as int }"); // 257 (bitop literales)
    // M28.3b: la asignación coerciona el literal al ancho del destino (var, campo, elemento).
    oracle_program("fn main() -> int { var x: u8 = 0; x = 200; x as int }");    // 200
    oracle_program("fn main() -> int { var a: [u32] = [0]; a[0] = 7; a[0] as int }"); // 7
}

/// M28.2: `?` con conversión de error vía `From<S>`. `expr?` (con `impl From<E1> for E2`) baja a
/// un `match` que convierte en la rama de error → runtime intacto, ambos motores coinciden.
#[test]
fn conversion_error_oracle() {
    let base = "enum MiErr { Io(string) } \
        impl From<string> for MiErr { fn convert(o: string) -> MiErr { MiErr.Io(o) } } \
        fn read(f: bool) -> Result<int, string> { if (f) { Result.Err(\"x\") } else { Result.Ok(7) } } \
        fn proc(f: bool) -> Result<int, MiErr> { let x = read(f)?; Result.Ok(x + 1) } ";
    // Camino Ok: proc(false) = Ok(8); code = 8.
    oracle_program(&format!("{base} fn main() -> int {{ match (proc(false)) {{ Result.Ok(v) => v, Result.Err(e) => 0 - 1 }} }}"));
    // Camino Err convertido: proc(true) = Err(MiErr.Io(\"x\")); se detecta la conversión → 99.
    oracle_program(&format!("{base} fn main() -> int {{ match (proc(true)) {{ Result.Ok(v) => v, Result.Err(e) => match (e) {{ MiErr.Io(s) => 99 }} }} }}"));
    // El `?` SIN conversión (mismo tipo de error) sigue intacto.
    oracle_program("fn read() -> Result<int, string> { Result.Ok(5) } fn proc() -> Result<int, string> { let x = read()?; Result.Ok(x * 2) } fn main() -> int { match (proc()) { Result.Ok(v) => v, Result.Err(e) => 0 } }");
}

/// Un `match` con TODOS los brazos divergentes (`return`) type-checkea (antes hacía panic el checker,
/// "hay al menos un brazo"): el match diverge y vale unit; la función retorna por los `return`.
#[test]
fn match_all_divergentes_oracle() {
    oracle_program("fn f(o: Option<int>) -> int { match (o) { Option.Some(n) => { return n; }, Option.None => { return 0; } } } fn main() -> int { f(Option.Some(5)) + f(Option.None) }"); // 5
}

/// M28.1: sobrecarga de operadores vía traits (`Add`/`Sub`/`Mul`/`Div`/`Neg`). `a op b` sobre
/// un tipo de usuario baja a `a.metodo(b)` (función manglada de M9) → ambos motores coinciden.
#[test]
fn operators_oracle() {
    let vec2 = "struct Vec2 { x: int, y: int } \
        impl Add for Vec2 { fn add(self, o: Vec2) -> Vec2 { Vec2 { x: self.x + o.x, y: self.y + o.y } } } \
        impl Sub for Vec2 { fn sub(self, o: Vec2) -> Vec2 { Vec2 { x: self.x - o.x, y: self.y - o.y } } } \
        impl Neg for Vec2 { fn neg(self) -> Vec2 { Vec2 { x: 0 - self.x, y: 0 - self.y } } } ";
    // Suma de vectores: (1,2) + (3,4) = (4,6) → 4+6 = 10.
    oracle_program(&format!("{vec2} fn main() -> int {{ let a = Vec2 {{ x: 1, y: 2 }}; let b = Vec2 {{ x: 3, y: 4 }}; let c = a + b; c.x + c.y }}"));
    // Resta y negación encadenadas: -((5,5) - (1,2)) = -(4,3) = (-4,-3) → -7.
    oracle_program(&format!("{vec2} fn main() -> int {{ let a = Vec2 {{ x: 5, y: 5 }}; let b = Vec2 {{ x: 1, y: 2 }}; let c = -(a - b); c.x + c.y }}"));
    // Suma triple encadenada (mismo operador, posición compartida en el AST): (1,0)+(1,0)+(1,0).
    oracle_program(&format!("{vec2} fn main() -> int {{ let u = Vec2 {{ x: 1, y: 0 }}; let s = u + u + u; s.x }}"));
    // Los operadores built-in sobre int/float siguen intactos (no se enrutan a traits).
    oracle_int("2 * 3 + 4");
}

#[test]
fn for_oracle() {
    oracle_program("fn main() -> int { var s = 0; for i in 0..5 { s = s + i; } s }"); // 10
    oracle_program("fn main() -> int { var t = 0; for x in [10, 20, 30] { t = t + x; } t }"); // 60
    oracle_program("fn main() -> int { var n = 0; for c in \"hello\" { n = n + 1; } n }"); // 4
    oracle_program("fn main() -> int { var m: Map<string, int> = Map.new(); m.insert(\"a\", 1); m.insert(\"b\", 5); var s = 0; for (k, v) in m { s = s + v; } s }"); // 6
    oracle_program("fn main() -> int { var m: Map<int, int> = Map.new(); m.insert(1, 10); m.insert(2, 20); var c = 0; for (k, _) in m { c = c + k; } c }"); // 3
    // for anidado.
    oracle_program("fn main() -> int { var s = 0; for i in 0..3 { for j in 0..3 { s = s + 1; } } s }"); // 9
    // `for` sobre un valor con return dentro (propaga).
    oracle_program("fn find(xs: [int], t: int) -> int { for x in xs { if (x == t) { return 1; } } 0 } fn main() -> int { find([3, 7, 9], 7) }"); // 1
}

#[test]
fn bitops_oracle() {
    // M19.3a: operadores bit a bit. Ambos motores comparten `wrapping_*` → idénticos.
    oracle_int("6 & 3");   // 0b110 & 0b011 = 0b010 = 2
    oracle_int("6 | 3");   // 0b111 = 7
    oracle_int("6 ^ 3");   // 0b101 = 5
    oracle_int("1 << 4");  // 16
    oracle_int("255 >> 4");// 15
    oracle_int("~0");      // -1 (complemento a uno)
    oracle_int("~5");      // -6
    // Precedencia: shift por debajo de aditivo; bit a bit por debajo de comparación.
    oracle_int("1 + 1 << 4");           // (1+1) << 4 = 32
    oracle_int("(0 | 1) & (2 | 1)");    // 1
    // Patrón típico de framing: combinar dos bytes en un entero de 16 bits.
    oracle_int("200 << 8 | 57");        // 200*256+57 = 51257
    // Máscara y desplazamiento encadenados (estilo extracción de campos).
    oracle_int("(51257 >> 8) & 255");   // 200
    oracle_int("51257 & 255");          // 57
}

/// M16.1a: el tipo `bytes` en el oráculo. Literal `b"..."` (con `\xNN`), `len`, indexar (→int) e
/// igualdad. Se enruta a `int` (el booleano de `==` vía `if`) porque `print(bytes)` está diferido.
#[test]
fn bytes_oracle() {
    oracle_int("b\"AB\".len()");                    // 2
    oracle_int("b\"hello\".len()");                  // 4
    oracle_int("b\"\\x00\\xff\"[1]");              // 255
    oracle_int("b\"AB\\x00\"[0]");                 // 65
    oracle_int("b\"AB\\x00\"[2]");                 // 0
    oracle_int("b\"\".len()");                      // 0 (vacío)
    // Igualdad estructural (misma secuencia / distinta) → 1/0.
    oracle_int("if (b\"AB\\xff\" == b\"AB\\xff\") { 1 } else { 0 }");
    oracle_int("if (b\"AB\" == b\"AC\") { 1 } else { 0 }");
    oracle_int("if (b\"AB\" == b\"ABC\") { 1 } else { 0 }");
    // Los caracteres no-ASCII se codifican como UTF-8 (á = 2 octetos).
    oracle_int("b\"á\".len()");                     // 2
    // M16.1b: to_bytes (builtin) + concatenación (opcode Add).
    oracle_int("\"hello, mundo\".to_bytes().len()");                   // 11
    oracle_int("\"á\".to_bytes().len()");                            // 2 (UTF-8)
    oracle_int("(\"AB\".to_bytes() + \"CD\".to_bytes()).len()");        // 4
    oracle_int("if (\"AB\".to_bytes() == b\"AB\") { 1 } else { 0 }");
    oracle_int("if (\"A\".to_bytes() + \"B\".to_bytes() == b\"AB\") { 1 } else { 0 }");
}

#[test]
fn bytes_a_hex_oracle() {
    // M16 (diferido): to_string(bytes) → hex en minúscula; idéntico en ambos motores.
    oracle_int("if (to_string(b\"Hi\\xff\") == \"4869ff\") { 1 } else { 0 }");   // H=48 i=69 ff
    oracle_int("if (to_string(b\"\\x00\\x01\\x02\") == \"000102\") { 1 } else { 0 }");
    oracle_int("if (to_string(b\"\") == \"\") { 1 } else { 0 }");                  // vacío
    oracle_int("if (to_string(\"raylang\".to_bytes()) == \"7261796c616e67\") { 1 } else { 0 }");
    oracle_int("to_string(b\"AB\\xff\").len()");                                    // 6 (2 hex por octeto)
}

/// M16.1b: `from_utf8` es un envoltorio del **prelude** (no un opcode), así que se prueba con el
/// oráculo a nivel de programa completo (que inyecta el prelude), no con expresiones sueltas.
#[test]
fn bytes_from_utf8_oracle() {
    // Round-trip válido: decodifica y mide la longitud del string.
    oracle_program("fn main() -> int { match (from_utf8(b\"hello\")) { Result.Ok(s) => s.len(), Result.Err(e) => -1, } }");
    // UTF-8 inválido → Err → 0.
    oracle_program("fn main() -> int { match (from_utf8(b\"\\xff\\xfe\")) { Result.Ok(s) => 1, Result.Err(e) => 0, } }");
    // to_bytes ∘ from_utf8 es identidad sobre texto válido.
    oracle_program("fn main() -> int { match (from_utf8(\"raylang\".to_bytes())) { Result.Ok(s) => s.len(), Result.Err(e) => -1, } }");
}

/// M19.2: `sub_bytes` (sub-secuencia por octeto, con clamp). Enrutado a int/bool (len/index/==),
/// como el resto de oráculos de bytes (print de bytes diferido).
#[test]
fn sub_bytes_oracle() {
    oracle_int("b\"hello\".sub_bytes(1, 4).len()");                       // 3 ("ell")
    oracle_int("b\"hello\".sub_bytes(1, 4)[0]");                          // 101 ('e')
    oracle_int("if (b\"ABCD\".sub_bytes(0, 2) == b\"AB\") { 1 } else { 0 }"); // 1
    oracle_int("if (b\"ABCD\".sub_bytes(2, 4) == b\"CD\") { 1 } else { 0 }"); // 1
    // Clamp: fin fuera de rango → recorta; inicio > n → vacío; i > j → vacío.
    oracle_int("b\"AB\".sub_bytes(0, 100).len()");                         // 2
    oracle_int("b\"AB\".sub_bytes(5, 10).len()");                          // 0
    oracle_int("b\"AB\".sub_bytes(1, 0).len()");                           // 0
    // Octetos crudos (incl. \x00/\xff) intactos.
    oracle_int("b\"\\x00\\xff\\x10\".sub_bytes(1, 2)[0]");               // 255
    oracle_int("b\"\\x00\\xff\\x10\".sub_bytes(0, 3).len()");             // 3
}

#[test]
fn bytes_of_oracle() {
    // M19.3c: construir bytes desde [int]. Indexar de vuelta da el mismo octeto.
    oracle_int("bytes_of([72, 105]).len()");                               // 2
    oracle_int("bytes_of([72, 105, 33])[1]");                             // 105
    // Truncado a octeto (`& 255`): 256 → 0, 511 → 255, negativos envuelven.
    oracle_int("bytes_of([256])[0]");                                     // 0
    oracle_int("bytes_of([511])[0]");                                     // 255
    // Round-trip con sub_bytes / igualdad de bytes.
    oracle_int("if (bytes_of([65, 66]) == b\"AB\") { 1 } else { 0 }");    // 1
    // Compone con concatenación de bytes (M16.1b): cabecera + carga.
    oracle_int("(bytes_of([129, 5]) + b\"hello\").len()");                   // 7
}

/// M13.1: Map en el oráculo. Las operaciones básicas dan el mismo resultado en ambos motores.
#[test]
fn map_basic_oracle() {
    oracle_program(
        "fn main() -> int {
            let m: Map<string, int> = Map.new();
            m.insert(\"a\", 1);
            m.insert(\"b\", 2);
            m.insert(\"a\", 10);
            let total = match (m.get(\"a\")) { Option.Some(v) => v, Option.None => 0 };
            total + m.len()
         }",
    );
}

/// P0.3: `add_to(m, k, delta)` — upsert acumulativo en 1 lookup (opcode `MapAdd`). Cubre conteo
/// (int), clave ausente (`m[k] = delta`), acumulación repetida y valores float. Oráculo VM↔intérprete.
#[test]
fn map_add_to_oracle() {
    oracle_program(
        "fn main() -> int {
            let m: Map<string, int> = Map.new();
            m.add_to(\"a\", 1);
            m.add_to(\"a\", 1);
            m.add_to(\"b\", 5);
            m.add_to(\"a\", 10);
            let f: Map<string, float> = Map.new();
            f.add_to(\"x\", 1.5);
            f.add_to(\"x\", 2.25);
            m.get_or(\"a\", 0) + m.get_or(\"b\", 0) + m.get_or(\"z\", 0) + m.len()
                + (if (f.get_or(\"x\", 0.0) > 3.7) { 100 } else { 0 })
         }",
    );
}

/// P0.6: la fusión `GetLocalConst;CmpJump` (guarda `local op const`) preserva la semántica de
/// `if`/`while` — incluida la **vuelta del bucle** (su `Jump` apunta al inicio de la condición, que
/// tras la fusión es la instrucción fusionada). Cubre guardas `<` (while) y `>` (if). Oráculo.
#[test]
fn guard_fusion_round3_oracle() {
    oracle_program(
        "fn main() -> int {
            var i = 0;
            var acc = 0;
            while (i < 100) {
                if (i > 50) { acc = acc + i; } else { acc = acc + 1; }
                i = i + 1;
            }
            acc
         }",
    );
}

/// M48.4d: los métodos de `StrOps`/`BytesOps` (trim/split/replace/…/sub_bytes) despachan por trait
/// y bajan a los builtins de string/bytes. Varios asignan heap → estrés del GC.
#[test]
fn trait_strops_bytesops_oracle() {
    oracle_stress(
        "fn main() -> int {
            let s = \"  Hola Mundo  \";
            let t = s.trim();
            let up = t.to_upper();
            let parts = t.split(\" \");
            let r = t.replace(\"Mundo\", \"Ray\");
            let cs = t.chars();
            let rep = \"xy\".repeat(4);
            let b = t.to_bytes();
            let sb = b.sub_bytes(0, 4);
            t.len() + up.len() + parts.len() + r.len() + cs.len() + rep.len()
                + b.len() + sb.len() + t.substring(0, 4).len()
                + (if (t.starts_with(\"Hola\")) { 1 } else { 0 })
                + (if (t.ends_with(\"Mundo\")) { 1 } else { 0 })
         }",
    );
}

/// M48.4c: `insert`/`contains_key`/`keys`/`values` como métodos del trait `MapOps` bajan a sus
/// primitivos `__x`. `keys`/`values` asignan heap y son deterministas (orden de clave) → oráculo.
#[test]
fn trait_mapops_oracle() {
    oracle_stress(
        "fn main() -> int {
            let m: Map<int, [int]> = [:];
            var i = 0;
            while (i < 20) { m.insert(i, [i, i * 3]); i = i + 1; }
            var sum = 0;
            let ks = m.keys();      // ordenadas 0..19
            let vs = m.values();    // en el mismo orden
            var j = 0;
            while (j < m.len()) { sum = sum + ks[j] + vs[j][1]; j = j + 1; }
            if (m.contains_key(7)) { sum = sum + 10000; }
            if (!m.contains_key(99)) { sum = sum + 100; }
            sum
         }",
    );
}

/// M48.4b: `push`/`reverse`/`contains` como métodos de trait (`Push`/`Reverse`/`Contains`) bajan a
/// sus primitivos `__x`. `push`/`reverse` asignan heap → estrés del GC.
#[test]
fn trait_push_reverse_contains_oracle() {
    oracle_stress(
        "struct Queue { items: [int] }
         impl Push<int> for Queue { fn push(self, x: int) { self.items.push(x) } }
         fn main() -> int {
            var a: [int] = [];
            var i = 0;
            while (i < 30) { a.push(i * 2); i = i + 1; }   // Push
            let r = a.reverse();                            // Reverse: [58, 56, …]
            let c = Queue { items: [7] };
            c.push(9);                                      // Push sobre tipo de usuario
            var sum = 0;
            if (a.contains(58)) { sum = sum + 1000; }     // Contains en arreglo
            if (\"abcdef\".contains(\"cde\")) { sum = sum + 100; } // Contains en string
            if (!a.contains(999)) { sum = sum + 10; }
            sum + a.len() + r[0] + c.items.len()           // 1110 + 30 + 58 + 2 = 1200
         }",
    );
}

/// M48.4a: `.len()` como método del trait `Len` (string/[T]/Map/bytes + tipo de usuario) baja al
/// primitivo `__len` (mismo opcode `Len`) → ambos motores coinciden.
#[test]
fn trait_len_oracle() {
    oracle_program(
        "struct Stack { d: [int] }
         impl Len for Stack { fn len(self) -> int { self.d.len() } }
         fn describe<T: Len>(x: T) -> int { x.len() }
         fn main() -> int {
            let m: Map<int, int> = [1: 10, 2: 20, 3: 30];
            let p = Stack { d: [7, 8, 9] };
            \"hello\".len() + [1,2,3,4,5].len() + m.len() + \"ab\".to_bytes().len()
                + p.len() + describe([1,2]) + describe(p)
         }",
    );
}

/// M48.2: el literal de Map `[k: v, …]` baja a `Map.new()` + `insert` por par → ambos motores
/// coinciden. Cubre poblado, `[:]` vacío, clave repetida (gana la última) y un valor con UFCS.
#[test]
fn map_literal_oracle() {
    oracle_program(
        "fn dup(x: int) -> int { x * 2 }
         fn main() -> int {
            let m = [1: 10, 2: 20, 1: 30];
            let empty: Map<int, int> = [:];
            empty.insert(9, dup(5));
            let a = match (m.get(1)) { Option.Some(v) => v, Option.None => 0 };
            let b = match (empty.get(9)) { Option.Some(v) => v, Option.None => 0 };
            a + b + m.len() + empty.len()
         }",
    );
}

/// M48.2: el literal de Map asigna en el heap (Map + valores) → estrés del GC. Un literal con
/// varios pares dentro de un bucle debe mantener sus valores vivos en cada recolección.
#[test]
fn map_literal_stress_gc_oracle() {
    oracle_stress(
        "fn main() -> int {
            var sum = 0;
            var i = 0;
            while (i < 20) {
                let m = [i: [i, i + 1], i + 100: [i + 2, i + 3]];
                match (m.get(i)) {
                    Option.Some(par) => { sum = sum + par[0] + par[1]; },
                    Option.None => { sum = sum - 1; },
                }
                i = i + 1;
            }
            sum
         }",
    );
}

/// M48.1: `Map.new()` (función asociada) baja al mismo opcode `MapNew` que el antiguo `map_new()`
/// → ambos motores coinciden. Mismo programa que `map_basico_oraculo` con la sintaxis nueva.
#[test]
fn map_new_associated_oracle() {
    oracle_program(
        "fn main() -> int {
            let m: Map<string, int> = Map.new();
            m.insert(\"a\", 1);
            m.insert(\"b\", 2);
            m.insert(\"a\", 10);
            let total = match (m.get(\"a\")) { Option.Some(v) => v, Option.None => 0 };
            total + m.len()
         }",
    );
}

/// M13.1: el Map asigna en el heap y guarda valores → estrés del GC (recolecta en cada paso).
/// Si una raíz faltara, los valores guardados se liberarían y el resultado cambiaría.
#[test]
fn map_stress_gc_oracle() {
    oracle_stress(
        "fn cell(n: int) -> [int] { [n, n * 2] }
         fn main() -> int {
            let m: Map<int, [int]> = Map.new();
            var i = 0;
            while (i < 30) { m.insert(i, cell(i)); i = i + 1; }
            var sum = 0;
            var j = 0;
            while (j < 30) {
                match (m.get(j)) {
                    Option.Some(par) => { sum = sum + par[0] + par[1]; },
                    Option.None => { sum = sum - 1; },
                }
                j = j + 1;
            }
            sum + m.len()
         }",
    );
}

/// M13.1: claves de distintos tipos primitivos hashables.
#[test]
fn map_keys_variadas_oracle() {
    oracle_program(
        "fn main() -> int {
            let byInt: Map<int, int> = Map.new();
            byInt.insert(7, 70);
            let byChar: Map<char, int> = Map.new();
            byChar.insert('z', 100);
            let byBool: Map<bool, int> = Map.new();
            byBool.insert(true, 1);
            byBool.insert(false, 2);
            let a = match (byInt.get(7)) { Option.Some(v) => v, Option.None => 0 };
            let b = match (byChar.get('z')) { Option.Some(v) => v, Option.None => 0 };
            let c = match (byBool.get(true)) { Option.Some(v) => v, Option.None => 0 };
            a + b + c + byBool.len()
         }",
    );
}

#[test]
fn map_bytes_key_oracle() {
    // M16 (diferido): `bytes` como clave de Map. Incluye octetos crudos (\x00/\xff).
    oracle_program(
        "fn main() -> int {
            let m: Map<bytes, int> = Map.new();
            m.insert(b\"one\", 10);
            m.insert(b\"\\x00\\xff\", 99);
            m.insert(b\"dos\", 20);
            let a = match (m.get(b\"one\")) { Option.Some(v) => v, Option.None => 0 };
            let b = match (m.get(b\"\\x00\\xff\")) { Option.Some(v) => v, Option.None => 0 };
            let c = if (m.contains_key(b\"dos\")) { 1 } else { 0 };
            a + b + c + m.len()
         }",
    );
}

#[test]
fn map_bytes_key_keys_oracle() {
    // keys/values con clave bytes: orden determinista (MapKey::Bytes es Ord lexicográfico).
    oracle_program(
        "fn main() -> int {
            let m: Map<bytes, int> = Map.new();
            m.insert(b\"c\", 3);
            m.insert(b\"a\", 1);
            m.insert(b\"b\", 2);
            let ks = m.keys();   // ordenadas: a, b, c
            let vs = m.values(); // 1, 2, 3
            var total = 0;
            var i = 0;
            while (i < vs.len()) { total = total + vs[i] * (i + 1); i = i + 1; }
            total + ks.len()
         }",
    );
}

/// M13.1b: keys (ordenadas) + values (en orden de clave) + remove, en el oráculo.
#[test]
fn map_keys_values_remove_oracle() {
    oracle_program(
        "fn sum(a: [int]) -> int { var s = 0; var i = 0; while (i < a.len()) { s = s + a[i]; i = i + 1; } s }
         fn main() -> int {
            let m: Map<int, int> = Map.new();
            m.insert(3, 30);
            m.insert(1, 10);
            m.insert(2, 20);
            let ks = m.keys();              // [1, 2, 3]
            let vs = m.values();            // [10, 20, 30]
            let removed = match (remove(m, 2)) { Option.Some(v) => v, Option.None => 0 };
            ks[0] * 100 + ks[2] + sum(vs) + removed + m.len()
         }",
    );
}

/// M13.1b: keys/values asignan arreglos en el heap → estrés del GC.
#[test]
fn map_keys_values_stress_gc_oracle() {
    oracle_stress(
        "fn sum(a: [int]) -> int { var s = 0; var i = 0; while (i < a.len()) { s = s + a[i]; i = i + 1; } s }
         fn main() -> int {
            let m: Map<int, int> = Map.new();
            var i = 0;
            while (i < 25) { m.insert(i, i * i); i = i + 1; }
            let total = sum(m.values()) + sum(m.keys());
            var quitados = 0;
            var j = 0;
            while (j < 25) {
                match (remove(m, j)) {
                    Option.Some(v) => { quitados = quitados + 1; },
                    Option.None => {},
                }
                j = j + 2;
            }
            total + quitados + m.len()
         }",
    );
}

/// M13.3b: recursión de cola PROFUNDA (más allá de MAX_FRAMES) funciona en ambos motores
/// gracias al TCO, y coinciden. Sin TCO, ambos cortarían en 1024 con desbordamiento.
#[test]
fn tco_deep_tail_recursion_oracle() {
    // 5000 > MAX_FRAMES (1024): solo pasa si la llamada en cola reutiliza el marco.
    oracle_program(
        "fn account(n: int, acc: int) -> int {
            if (n == 0) { acc } else { account(n - 1, acc + 1) }
         }
         fn main() -> int { account(5000, 0) }",
    );
}

/// M13.3b: recursión mutua en cola + `return` en cola, también profunda.
#[test]
fn tco_mutual_and_return_in_tail_oracle() {
    oracle_program(
        "fn par(n: int) -> bool { if (n == 0) { true } else { return impar(n - 1); } }
         fn impar(n: int) -> bool { if (n == 0) { false } else { par(n - 1) } }
         fn main() -> int { if (par(4000)) { 1 } else { 0 } }",
    );
}

/// M13.3b: una llamada que NO está en cola (su valor se usa en `n + ...`) sigue recurriendo de
/// verdad —el TCO no debe convertirla— y da el mismo resultado en ambos motores. La profundidad
/// es modesta porque el intérprete recurre sobre la pila de Rust (el hilo de test es pequeño; el
/// binario real corre con pila grande, M13.3a). Que la recursión de cola SÍ se optimiza lo
/// prueban `tco_recursion_de_cola_profunda_oraculo` (5000) y `tco_mutua_*` (4000).
#[test]
fn tco_does_not_apply_to_non_tail_call_oracle() {
    oracle_program(
        "fn sum_hasta(n: int) -> int { if (n == 0) { 0 } else { n + sum_hasta(n - 1) } }
         fn main() -> int { sum_hasta(30) }",
    );
}

#[test]
fn local_variables_and_shadowing() {
    oracle_program("fn main() -> int { let x: int = 1; { let x: int = 99; } x }");
    oracle_program(
        "fn main() -> int { var s: int = 0; var i: int = 0; while (i < 5) { s = s + i; i = i + 1; } s }",
    );
}

#[test]
fn program_con_print() {
    oracle_program("fn main() -> int { print(42); print(true); 0 }");
}

// ----- M3.1: arreglos -----

#[test]
fn arrays_index_len_y_sum() {
    oracle_program("fn main() -> int { let a: [int] = [10, 20, 30]; a[0] + a[2] }");
    oracle_program("fn main() -> int { let a: [int] = [1, 2, 3, 4]; a.len() }");
}

#[test]
fn arrays_mutation_y_push() {
    oracle_program("fn main() -> int { var a: [int] = [1, 2, 3]; a[1] = 99; a[1] }");
    oracle_program(
        "fn main() -> int { let a: [int] = []; a.push(5); a.push(7); a[0] + a[1] }",
    );
}

#[test]
fn arrays_are_by_reference() {
    oracle_program("fn main() -> int { let a: [int] = [1, 2, 3]; let b: [int] = a; b[0] = 9; a[0] }");
}

#[test]
fn array_sum_with_while() {
    oracle_program(
        "fn sum(a: [int]) -> int {
            var s: int = 0; var i: int = 0;
            while (i < a.len()) { s = s + a[i]; i = i + 1; }
            s
         }
         fn main() -> int { sum([5, 10, 15, 20]) }",
    );
}

#[test]
fn index_out_of_range_is_error() {
    let prog_src = "fn main() -> int { let a: [int] = [1, 2]; a[5] }";
    let tokens = crate::lexer::lex(prog_src).unwrap();
    let mut prog = crate::parser::parse(tokens).unwrap();
    crate::checker::check(&mut prog).unwrap();
    let compiled = compile_program(&prog).unwrap();
    assert!(run_program(&compiled).unwrap_err().msg.contains("out of range"));
}

// ----- M3.2: structs -----

#[test]
fn struct_field_access_and_order() {
    oracle_program("struct P { x: int, y: int } fn main() -> int { let p: P = P { x: 3, y: 4 }; p.x + p.y }");
    oracle_program("struct P { x: int, y: int } fn main() -> int { let p: P = P { y: 4, x: 3 }; p.x - p.y }");
}

#[test]
fn structs_field_mutation() {
    oracle_program("struct P { x: int, y: int } fn main() -> int { let p: P = P { x: 1, y: 2 }; p.x = 9; p.x + p.y }");
}

#[test]
fn structs_are_by_reference() {
    oracle_program("struct C { v: int } fn main() -> int { let a: C = C { v: 1 }; let b: C = a; b.v = 9; a.v }");
}

#[test]
fn structs_nested_vars_y_con_arrays() {
    oracle_program(
        "struct P { x: int, y: int }
         struct L { a: P, b: P }
         fn dx(l: L) -> int { l.b.x - l.a.x }
         fn main() -> int { dx(L { a: P { x: 1, y: 0 }, b: P { x: 5, y: 0 } }) }",
    );
    oracle_program(
        "struct Stack { data: [int] }
         fn main() -> int { let s: Stack = Stack { data: [10, 20] }; s.data.push(30); s.data[2] }",
    );
}

// ----- M4.1: funciones de primera clase -----

#[test]
fn anonymous_function_in_variable() {
    oracle_program("fn main() -> int { let f: fn(int) -> int = fn(x: int) -> int { x * x }; f(9) }");
}

#[test]
fn higher_order_receives_function() {
    oracle_program(
        "fn apply(f: fn(int) -> int, x: int) -> int { f(x) }
         fn main() -> int { apply(fn(n: int) -> int { n + 1 }, 41) }",
    );
}

#[test]
fn function_name_as_value() {
    oracle_program(
        "fn inc(n: int) -> int { n + 1 }
         fn apply(f: fn(int) -> int, x: int) -> int { f(x) }
         fn main() -> int { apply(inc, 10) }",
    );
}

#[test]
fn returning_a_function() {
    oracle_program(
        "fn choose(b: bool) -> fn(int) -> int {
             if (b) { fn(n: int) -> int { n + n } } else { fn(n: int) -> int { n * n } }
         }
         fn main() -> int { let f: fn(int) -> int = choose(true); f(21) }",
    );
}

#[test]
fn call_a_function_literal_directly() {
    oracle_program("fn main() -> int { (fn(x: int) -> int { x + x })(21) }");
}

#[test]
fn variable_shadows_global_function() {
    oracle_program(
        "fn f(x: int) -> int { x * 100 }
         fn main() -> int { let f: fn(int) -> int = fn(x: int) -> int { x + 1 }; f(41) }",
    );
}

#[test]
fn map_over_about_array_con_function() {
    oracle_program(
        "fn map_over(a: [int], f: fn(int) -> int) {
             var i: int = 0;
             while (i < a.len()) { a[i] = f(a[i]); i = i + 1; }
         }
         fn main() -> int {
             var xs: [int] = [1, 2, 3, 4];
             map_over(xs, fn(n: int) -> int { n * n });
             xs[0] + xs[1] + xs[2] + xs[3]
         }",
    );
}

// ----- M4.2: closures (captura de entorno) -----

#[test]
fn closure_capture_un_let() {
    oracle_program(
        "fn main() -> int {
             let base: int = 1000;
             let f: fn(int) -> int = fn(d: int) -> int { base + d };
             f(7)
         }",
    );
}

#[test]
fn counter_with_mutable_state() {
    oracle_program(
        "fn counter() -> fn() -> int { var n: int = 0; fn() -> int { n = n + 1; n } }
         fn main() -> int { let c: fn() -> int = counter(); c(); c(); c() }",
    );
}

#[test]
fn closure_instances_are_independent() {
    oracle_program(
        "fn counter() -> fn() -> int { var n: int = 0; fn() -> int { n = n + 1; n } }
         fn main() -> int {
             let a: fn() -> int = counter();
             let b: fn() -> int = counter();
             a(); a(); a();   // n de a -> 3
             b();             // n de b -> 1 (su propia celda, independiente)
             a() + b()        // a()->4, b()->2 => 6
         }",
    );
}

#[test]
fn transitive_capture_two_levels() {
    oracle_program(
        "fn adder(x: int) -> fn(int) -> int { fn(y: int) -> int { x + y } }
         fn main() -> int { let add5: fn(int) -> int = adder(5); add5(10) + add5(100) }",
    );
}

#[test]
fn sibling_closures_share_cell() {
    oracle_program(
        "struct Par { inc: fn(), get: fn() -> int }
         fn make() -> Par {
             var n: int = 0;
             Par { inc: fn() { n = n + 1; }, get: fn() -> int { n } }
         }
         fn main() -> int { let p: Par = make(); p.inc(); p.inc(); p.inc(); p.get() }",
    );
}

#[test]
fn closure_en_array_y_order_superior() {
    oracle_program(
        "fn applies_dos(f: fn(int) -> int, x: int) -> int { f(f(x)) }
         fn main() -> int {
             let k: int = 3;
             applies_dos(fn(n: int) -> int { n + k }, 10)
         }",
    );
}

// ----- M5.1: enums (tipos suma) y construcción -----

#[test]
fn enum_construction_oracle() {
    // Ambos motores construyen variantes (con y sin payload) y coinciden en el
    // resultado. El payload se evalúa en orden antes de MakeEnum.
    oracle_program(
        "enum E { A(int, int), B }
         fn main() -> int { let x: E = E.A(2, 3); let y: E = E.B; print(x); print(y); 0 }",
    );
}

#[test]
fn enum_recursive_oracle() {
    oracle_program(
        "enum List { Cons(int, List), Nil }
         fn main() -> int { let xs: List = List.Cons(1, List.Cons(2, List.Nil)); print(xs); 0 }",
    );
}

#[test]
fn derive_show_oracle() {
    // `@derive(Show)` genera `mostrar` (front-end → impls normales): el intérprete y la VM
    // deben producir la **misma** cadena. Se compara vía `len` (el oráculo mira el retorno).
    oracle_program(
        "@derive(Show)
         enum Color { Rojo, RGB(int, int, int) }
         @derive(Show)
         struct Point { x: int, y: int }
         fn main() -> int {
             let p = Point { x: 3, y: 40 };
             print(p.show());
             print(Color.RGB(1, 2, 3).show());
             p.show().len() + Color.RGB(1, 2, 3).show().len()
         }",
    );
}

#[test]
fn enums_en_mode_stress() {
    // Construir enums (incl. recursivos) con el GC recolectando en cada punto
    // seguro: si el trazado del payload faltara, un valor vivo se liberaría.
    oracle_stress(
        "enum List { Cons(int, List), Nil }
         fn build(n: int) -> List {
             if (n == 0) { List.Nil } else { List.Cons(n, build(n - 1)) }
         }
         fn main() -> int { let xs: List = build(20); print(xs); 0 }",
    );
}

#[test]
fn gc_frees_unreachable_enums() {
    // Cada llamada construye una lista enlazada que queda inalcanzable al
    // retornar. El mark-and-sweep debe barrer esos objetos de enum: el heap
    // queda acotado en vez de crecer sin parar.
    let src = r#"
        enum List { Cons(int, List), Nil }
        fn build(n: int) -> List {
            if (n == 0) { List.Nil } else { List.Cons(n, build(n - 1)) }
        }
        fn main() -> int {
            var i: int = 0;
            while (i < 50) { let xs: List = build(10); i = i + 1; }
            0
        }
    "#;
    let tokens = crate::lexer::lex(src).unwrap();
    let mut prog = crate::parser::parse(tokens).unwrap();
    crate::checker::check(&mut prog).unwrap();
    let compiled = compile_program(&prog).unwrap();
    let mut vm = Vm::new(&compiled);
    vm.run().expect("vm ok");
    // Sin GC habría ~550 objetos vivos; con barrido, muy pocos.
    assert!(vm.cur.heap.live() < 80, "el heap no se acotó: {} objetos vivos", vm.cur.heap.live());
}

// ----- M5.3: match en la VM (oráculo VM<->intérprete) -----

#[test]
fn match_traversal_oracle() {
    // Recorrer un enum recursivo con match: longitud y suma, en ambos motores.
    oracle_program(
        "enum List { Cons(int, List), Nil }
         fn length(xs: List) -> int { match (xs) { List.Cons(_, t) => 1 + length(t), List.Nil => 0 } }
         fn sum(xs: List) -> int { match (xs) { List.Cons(h, t) => h + sum(t), List.Nil => 0 } }
         fn main() -> int {
             let xs: List = List.Cons(10, List.Cons(20, List.Cons(30, List.Nil)));
             length(xs) * 100 + sum(xs)
         }",
    );
}

#[test]
fn match_selects_branch_oracle() {
    // Variantes con distinta aridad de payload; cada brazo liga lo suyo.
    oracle_program(
        "enum Shape { Circulo(int), Rect(int, int), Punto }
         fn area(f: Shape) -> int {
             match (f) { Shape.Circulo(r) => 3 * r * r, Shape.Rect(w, h) => w * h, Shape.Punto => 0 }
         }
         fn main() -> int { area(Shape.Rect(4, 5)) + area(Shape.Circulo(2)) + area(Shape.Punto) }",
    );
}

#[test]
fn match_wildcard_and_binding_oracle() {
    // Comodín `_` (dentro de variante y suelto) y binding catch-all.
    oracle_program(
        "enum E { Uno, Dos, Otro }
         fn n(e: E) -> int { match (e) { E.Uno => 1, other => 99 } }
         fn main() -> int { n(E.Uno) * 100 + n(E.Dos) }",
    );
}

#[test]
fn match_en_mode_stress() {
    // La prueba clave de M5.3: con el GC recolectando en CADA punto seguro, el
    // escrutinio guardado en el local temporal y el payload extraído deben seguir
    // rooteados. Si faltara una raíz, recorrer la lista reventaría o cambiaría.
    oracle_stress(
        "enum List { Cons(int, List), Nil }
         fn build(n: int) -> List { if (n == 0) { List.Nil } else { List.Cons(n, build(n - 1)) } }
         fn sum(xs: List) -> int { match (xs) { List.Cons(h, t) => h + sum(t), List.Nil => 0 } }
         fn main() -> int { sum(build(15)) }",
    );
}

#[test]
fn match_binding_captured_by_closure_oracle() {
    // Interacción fina: un binding de match capturado por una closure debe
    // BOXEARSE (vivir en una celda). InitLocal sobre el slot del binding lo
    // maneja, igual que con un `let`. Ambos motores deben coincidir.
    oracle_program(
        "enum E { A(int), B(int), C }
         fn adder(e: E) -> fn(int) -> int {
             match (e) {
                 E.A(n) => fn(x: int) -> int { x + n },
                 E.B(n) => fn(x: int) -> int { x * n },
                 E.C    => fn(x: int) -> int { x },
             }
         }
         fn main() -> int {
             let f: fn(int) -> int = adder(E.A(10));
             let g: fn(int) -> int = adder(E.B(3));
             f(5) + g(5)
         }",
    );
}

#[test]
fn match_nested_in_expressions_oracle() {
    // match como expresión: su valor alimenta otra operación, y el cuerpo de un
    // brazo construye otra variante (resolución dentro del brazo).
    oracle_program(
        "enum Sem { Rojo, Verde }
         fn opposite(s: Sem) -> Sem { match (s) { Sem.Rojo => Sem.Verde, Sem.Verde => Sem.Rojo } }
         fn a_int(s: Sem) -> int { match (s) { Sem.Rojo => 0, Sem.Verde => 1 } }
         fn main() -> int { a_int(opposite(Sem.Rojo)) + a_int(opposite(Sem.Verde)) * 10 }",
    );
}

// ----- M6.1: funciones genéricas (erasure: ambos motores coinciden) -----

#[test]
fn generic_identity_oracle() {
    // Con borrado de tipos, una función genérica solo mueve valores: el resultado
    // debe coincidir en intérprete y VM sin que el runtime sepa nada de T.
    oracle_program(
        "fn identity<T>(x: T) -> T { x }
         fn main() -> int { let b: bool = identity(true); let n: int = identity(7); if (b) { n } else { 0 } }",
    );
}

#[test]
fn higher_order_generic_oracle() {
    oracle_program(
        "fn apply<T, U>(f: fn(T) -> U, x: T) -> U { f(x) }
         fn double(n: int) -> int { n * 2 }
         fn main() -> int { apply(double, 21) }",
    );
}

#[test]
fn generic_about_arrays_oracle() {
    oracle_program(
        "fn par<T>(a: T, b: T) -> [T] { [a, b] }
         fn main() -> int { let xs: [int] = par(10, 32); xs[0] + xs[1] }",
    );
}

// ----- M6.2: tipos genéricos del usuario (erasure: ambos motores coinciden) -----

#[test]
fn enum_generic_oracle() {
    oracle_program(
        "enum Box<T> { Llena(T), Vacia }
         fn val(c: Box<int>, def: int) -> int { match (c) { Box.Llena(v) => v, Box.Vacia => def } }
         fn main() -> int {
             let a: Box<int> = Box.Llena(7);
             let b: Box<int> = Box.Vacia;
             val(a, 0) + val(b, 35)
         }",
    );
}

#[test]
fn struct_generic_oracle() {
    oracle_program(
        "struct Par<A, B> { first: A, second: B }
         fn main() -> int {
             let p: Par<int, bool> = Par { first: 10, second: true };
             if (p.second) { p.first } else { 0 }
         }",
    );
}

// ----- M6.3: Option/Result y el operador ? (oráculo) -----

#[test]
fn try_result_oracle() {
    oracle_program(
        "fn d(a: int, b: int) -> Result<int, string> { if (b == 0) { Result.Err(\"cero\") } else { Result.Ok(a / b) } }
         fn calc(x: int, y: int, z: int) -> Result<int, string> { let q1: int = d(x, y)?; let q2: int = d(q1, z)?; Result.Ok(q1 + q2) }
         fn unwrap(r: Result<int, string>) -> int { match (r) { Result.Ok(v) => v, Result.Err(_) => -1 } }
         fn main() -> int { unwrap(calc(100, 5, 2)) * 100 + unwrap(calc(100, 0, 2)) }",
    );
}

#[test]
fn try_option_oracle() {
    oracle_program(
        "fn first(xs: [int]) -> Option<int> { if (xs.len() == 0) { Option.None } else { Option.Some(xs[0]) } }
         fn mas_one(xs: [int]) -> Option<int> { let v: int = first(xs)?; Option.Some(v + 1) }
         fn unwrap(o: Option<int>) -> int { match (o) { Option.Some(v) => v, Option.None => -99 } }
         fn main() -> int { unwrap(mas_one([41])) * 100 + unwrap(mas_one([])) }",
    );
}

#[test]
fn try_en_mode_stress() {
    // El ? construye/propaga valores de enum (Result) bajo el GC en cada punto
    // seguro: el escrutinio del ? vive en su local temporal y queda rooteado.
    oracle_stress(
        "fn d(a: int, b: int) -> Result<int, string> { if (b == 0) { Result.Err(\"cero\") } else { Result.Ok(a / b) } }
         fn chain(n: int) -> Result<int, string> { let a: int = d(n, 2)?; let b: int = d(a, 1)?; Result.Ok(a + b) }
         fn unwrap(r: Result<int, string>) -> int { match (r) { Result.Ok(v) => v, Result.Err(_) => -1 } }
         fn main() -> int { unwrap(chain(40)) }",
    );
}

#[test]
fn enum_generic_recursive_en_stress() {
    // Lista genérica construida con un tipo concreto, recorrida con match, bajo el
    // GC en modo estrés: los valores de enum genérico se trazan como cualquier enum.
    oracle_stress(
        "enum List<T> { Cons(T, List<T>), Nil }
         fn sum(xs: List<int>) -> int { match (xs) { List.Cons(h, t) => h + sum(t), List.Nil => 0 } }
         fn build(n: int) -> List<int> { if (n == 0) { List.Nil } else { List.Cons(n, build(n - 1)) } }
         fn main() -> int { sum(build(15)) }",
    );
}

// ----- M4.3: recolección de basura -----

#[test]
fn el_gc_no_breaks_programas_en_mode_stress() {
    // Si el GC liberara algo vivo (raíz faltante), estos resultados cambiarían.
    oracle_stress("fn fib(n: int) -> int { if (n < 2) { n } else { fib(n-1) + fib(n-2) } } fn main() -> int { fib(12) }");
    oracle_stress(
        "fn main() -> int {
             var xs: [int] = [];
             var i: int = 0;
             while (i < 30) { xs.push(i * i); i = i + 1; }
             var s: int = 0; var j: int = 0;
             while (j < xs.len()) { s = s + xs[j]; j = j + 1; }
             s
         }",
    );
    oracle_stress(
        "struct P { x: int, y: int }
         fn main() -> int { var p: P = P { x: 1, y: 2 }; p.x = 10; p.x + p.y }",
    );
    oracle_stress(
        "fn counter() -> fn() -> int { var n: int = 0; fn() -> int { n = n + 1; n } }
         fn main() -> int { let c: fn() -> int = counter(); c(); c(); c(); c() }",
    );
}

#[test]
fn gc_frees_cycles() {
    // Cada 'make_cycle' crea un ciclo (celda <-> closure) que queda inalcanzable
    // al retornar. Con conteo de referencias se filtrarían (~200 objetos); el
    // mark-and-sweep los libera, así que el heap queda acotado.
    let src = r#"
        fn make_cycle() {
            var f: fn() = fn() {};
            f = fn() { f(); };
        }
        fn main() -> int {
            var i: int = 0;
            while (i < 100) { make_cycle(); i = i + 1; }
            0
        }
    "#;
    let tokens = crate::lexer::lex(src).unwrap();
    let mut prog = crate::parser::parse(tokens).unwrap();
    crate::checker::check(&mut prog).unwrap();
    let compiled = compile_program(&prog).unwrap();
    let mut vm = Vm::new(&compiled);
    vm.run().expect("vm ok");
    // Sin GC habría ~200 objetos vivos; con mark-and-sweep, muy pocos.
    assert!(vm.cur.heap.live() < 80, "el heap no se acotó: {} objetos vivos", vm.cur.heap.live());
}

// ----- M7.1: UFCS (azúcar de front-end; ambos motores ven la llamada ya bajada) -----

#[test]
fn ufcs_oracle() {
    // Función del usuario y builtin (len) usados como métodos.
    oracle_program(r#"
        fn sum(a: int, b: int) -> int { a + b }
        fn main() -> int {
            let xs: [int] = [1, 2, 3, 4];
            let n: int = xs.len();      // len(xs) = 4
            let v: int = 10;
            v.sum(n)                    // suma(10, 4) = 14
        }
    "#);
}

#[test]
fn ufcs_chained_oracle() {
    oracle_program(r#"
        fn double(x: int) -> int { x * 2 }
        fn inc(x: int) -> int { x + 1 }
        fn main() -> int {
            let v: int = 5;
            v.double().inc().double()      // doble(inc(doble(5))) = 22
        }
    "#);
}

#[test]
fn ufcs_about_struct_oracle() {
    // 'norma1' no es campo de Punto -> UFCS; 'p.x' sigue siendo acceso a campo.
    oracle_program(r#"
        struct Point { x: int, y: int }
        fn norma1(p: Point) -> int { p.x + p.y }
        fn main() -> int {
            let p: Point = Point { x: 7, y: 6 };
            p.norma1() + p.x             // 13 + 7 = 20
        }
    "#);
}

#[test]
fn ufcs_field_function_oracle() {
    // 'op' ES un campo de tipo función: c.op(x) llama al campo, no es UFCS.
    oracle_program(r#"
        struct Box { op: fn(int) -> int }
        fn main() -> int {
            let c: Box = Box { op: fn(x: int) -> int { x + 100 } };
            c.op(41)                     // (c.op)(41) = 141
        }
    "#);
}

#[test]
fn ufcs_en_mode_stress() {
    // El receptor y los argumentos viven en el heap: el GC en estrés no debe
    // romper la llamada UFCS bajada.
    oracle_stress(r#"
        fn head(xs: [int]) -> int { xs[0] }
        fn tail_sum(xs: [int]) -> int {
            var s: int = 0;
            var i: int = 1;
            while (i < xs.len()) { s = s + xs[i]; i = i + 1; }
            s
        }
        fn main() -> int {
            let xs: [int] = [10, 20, 30, 40];
            xs.head() + xs.tail_sum()   // 10 + 90 = 100
        }
    "#);
}

// ----- M7.2: pipelines (azúcar de parser; ambos motores ven la llamada bajada) -----

#[test]
fn pipeline_oracle() {
    oracle_program(r#"
        fn double(x: int) -> int { x * 2 }
        fn inc(x: int) -> int { x + 1 }
        fn sum(a: int, b: int) -> int { a + b }
        fn main() -> int {
            let v: int = 5;
            let a: int = v |> double |> inc;   // inc(doble(5)) = 11
            let b: int = v |> sum(100);       // suma(5, 100) = 105
            a + b                               // 116
        }
    "#);
}

#[test]
fn pipeline_y_ufcs_oracle() {
    // `.f()` (UFCS) y `|> f` (pipeline) componen sobre el mismo valor.
    oracle_program(r#"
        fn double(x: int) -> int { x * 2 }
        fn inc(x: int) -> int { x + 1 }
        fn main() -> int {
            let v: int = 5;
            v.double() |> inc |> double           // doble(inc(doble(5))) = 22
        }
    "#);
}

#[test]
fn pipeline_en_mode_stress() {
    // El valor que fluye por el pipeline es un arreglo en el heap.
    oracle_stress(r#"
        fn sum_todo(xs: [int]) -> int {
            var s: int = 0;
            var i: int = 0;
            while (i < xs.len()) { s = s + xs[i]; i = i + 1; }
            s
        }
        fn con_extra(xs: [int], x: int) -> [int] { xs.push(x); xs }
        fn main() -> int {
            let xs: [int] = [1, 2, 3];
            xs |> con_extra(4) |> sum_todo     // suma_todo(con_extra(xs, 4)) = 10
        }
    "#);
}

// ----- M7.3: stdlib (prelude map/filter/fold escrito en raylang) -----

#[test]
fn prelude_map_filter_fold_oracle() {
    oracle_program(r#"
        fn double(x: int) -> int { x * 2 }
        fn par(x: int) -> bool { x % 2 == 0 }
        fn sum(a: int, b: int) -> int { a + b }
        fn main() -> int {
            let xs: [int] = [1, 2, 3, 4, 5];
            let ys: [int] = xs.map(double).filter(par);  // [2,4,6,8,10]
            ys.fold(0, sum)                             // 30
        }
    "#);
}

#[test]
fn prelude_pipeline_oracle() {
    // El mismo cálculo, en estilo pipeline.
    oracle_program(r#"
        fn double(x: int) -> int { x * 2 }
        fn par(x: int) -> bool { x % 2 == 0 }
        fn sum(a: int, b: int) -> int { a + b }
        fn main() -> int {
            let xs: [int] = [1, 2, 3, 4, 5];
            xs |> filter(par) |> map(double) |> fold(0, sum)  // [2,4]->[4,8]->12
        }
    "#);
}

#[test]
fn prelude_con_closures_oracle() {
    // map/fold con funciones anónimas inline.
    oracle_program(r#"
        fn main() -> int {
            let xs: [int] = [1, 2, 3, 4];
            let squares: [int] = xs |> map(fn(x: int) -> int { x * x });  // [1,4,9,16]
            squares.fold(0, fn(a: int, x: int) -> int { a + x })           // 30
        }
    "#);
}

#[test]
fn prelude_en_mode_stress() {
    // map y filter alojan arreglos nuevos en el heap: el GC en estrés debe
    // mantenerlos vivos durante toda la cadena.
    oracle_stress(r#"
        fn inc(x: int) -> int { x + 1 }
        fn pos(x: int) -> bool { x > 3 }
        fn sum(a: int, b: int) -> int { a + b }
        fn main() -> int {
            let xs: [int] = [1, 2, 3, 4, 5, 6];
            xs.map(inc).filter(pos).fold(0, sum)   // [2..7]->[4,5,6,7]->22
        }
    "#);
}

// ----- M8.1: inferencia local (solo checker; el runtime no cambia) -----

#[test]
fn local_inference_oracle() {
    // Variables inferidas (int, [int], struct, enum genérico) deben dar el mismo
    // resultado en ambos motores: la inferencia se borra antes de ejecutar.
    oracle_program(r#"
        struct Point { x: int, y: int }
        enum Box<T> { Llena(T), Vacia }
        fn double(x: int) -> int { x * 2 }
        fn main() -> int {
            let x = 3;
            let xs = [10, 20, 30];
            let p = Point { x: 7, y: 6 };
            let c = Box.Llena(5);
            var total = 0;
            total = total + x.double();
            let inside = match (c) { Box.Llena(v) => v, Box.Vacia => 0 };
            total + xs[0] + p.x + p.y + inside   // 6 + 10 + 7 + 6 + 5 = 34
        }
    "#);
}

// ----- M9.1: traits (erasure; ambos motores ven funciones y llamadas ordinarias) -----

#[test]
fn traits_static_dispatch_oracle() {
    // Un trait implementado para un struct, un enum y un primitivo: los métodos se
    // bajan a funciones mangladas y las llamadas por punto a llamadas ordinarias,
    // así que la VM y el intérprete deben coincidir sin tocar el runtime.
    oracle_program(r#"
        trait Value { fn value(self) -> int; }
        struct Point { x: int, y: int }
        enum Coin { Cara, Cruz }
        impl Value for Point { fn value(self) -> int { self.x + self.y } }
        impl Value for Coin {
            fn value(self) -> int { match (self) { Coin.Cara => 1, Coin.Cruz => 0 } }
        }
        impl Value for int { fn value(self) -> int { self } }
        fn main() -> int {
            let p = Point { x: 3, y: 4 };
            p.value() + Coin.Cara.value() + 10.value()   // 7 + 1 + 10 = 18
        }
    "#);
}

#[test]
fn traits_self_and_internal_methods_oracle() {
    // `Self` en el retorno, parámetros extra, y un método que llama a otro del mismo
    // impl (`self.sumar(self)`): bajo estrés del GC para validar las raíces.
    oracle_stress(r#"
        trait Punteable {
            fn add(self, other: Point) -> Point;
            fn double(self) -> Self;
            fn norma(self) -> int;
        }
        struct Point { x: int, y: int }
        impl Punteable for Point {
            fn add(self, other: Point) -> Point { Point { x: self.x + other.x, y: self.y + other.y } }
            fn double(self) -> Self { self.add(self) }
            fn norma(self) -> int { self.x * self.x + self.y * self.y }
        }
        fn main() -> int {
            let p = Point { x: 3, y: 4 };
            p.double().norma()   // (6,8) -> 36 + 64 = 100
        }
    "#);
}

// ----- M9.2: bounds vía paso de diccionarios -----

#[test]
fn bounds_dictionaries_oracle() {
    // Genérico acotado sobre struct y primitivo + reenvío entre genéricos. Los
    // diccionarios son valores función; ambos motores deben coincidir.
    oracle_program(r#"
        trait Value { fn value(self) -> int; }
        struct Point { x: int, y: int }
        impl Value for Point { fn value(self) -> int { self.x + self.y } }
        impl Value for int { fn value(self) -> int { self } }
        fn double_value<T: Value>(x: T) -> int { x.value() + x.value() }
        fn sum_three<T: Value>(a: T, b: T, c: T) -> int {
            double_value(a) + b.value() + c.value()   // reenvío del diccionario
        }
        fn main() -> int {
            let p = Point { x: 3, y: 4 };
            double_value(p) + double_value(10) + sum_three(p, p, p)   // 14 + 20 + 28 = 62
        }
    "#);
}

#[test]
fn bounds_multiples_oracle() {
    // T: A + B — dos diccionarios. Bajo estrés del GC.
    oracle_stress(r#"
        trait Name { fn largo(self) -> int; }
        trait Double { fn double(self) -> int; }
        struct Thing { n: int }
        impl Name for Thing { fn largo(self) -> int { self.n } }
        impl Double for Thing { fn double(self) -> int { self.n + self.n } }
        fn usar<T: Name + Double>(x: T) -> int { x.largo() + x.double() }
        fn main() -> int {
            let c = Thing { n: 5 };
            usar(c)   // 5 + 10 = 15
        }
    "#);
}

// ----- M9.2b: impls genéricos -----

#[test]
fn generic_impl_without_bounds_oracle() {
    // `impl<T> Trait for Caja<T>` cuyo método no usa T: el método manglado es genérico
    // pero sin diccionarios. Despacha igual para Caja<int> y Caja<string>.
    oracle_program(r#"
        struct Box<T> { contenido: T }
        trait Count { fn count(self) -> int; }
        impl<T> Count for Box<T> { fn count(self) -> int { 1 } }
        fn main() -> int {
            let c = Box { contenido: 42 };
            let s = Box { contenido: "hello" };
            c.count() + s.count()   // 1 + 1 = 2
        }
    "#);
}

#[test]
fn impl_generic_bounded_direct_call_oracle() {
    // `impl<T: Mostrable> Mostrable for Caja<T>`: el cuerpo usa T.show() (vía el
    // diccionario interno). Llamada directa sobre Caja<int> → el dict interno es el de
    // int (plano). Es M9.2b-1: el caso anidado (pasar Caja a otro genérico) es -2.
    oracle_stress(r#"
        struct Box<T> { contenido: T }
        trait Measure { fn measure(self) -> int; }
        impl Measure for int { fn measure(self) -> int { self } }
        impl<T: Measure> Measure for Box<T> { fn measure(self) -> int { self.contenido.measure() + 1 } }
        fn main() -> int {
            let c = Box { contenido: 41 };
            c.measure()   // 41 + 1 = 42
        }
    "#);
}

#[test]
fn impl_generic_nested_dictionary_oracle() {
    // M9.2b-2: pasar un Caja<int> a otro genérico acotado. El diccionario de Caja<int> es
    // un **closure** que captura el de int. Ambos motores deben coincidir.
    oracle_program(r#"
        struct Box<T> { contenido: T }
        trait Measure { fn measure(self) -> int; }
        impl Measure for int { fn measure(self) -> int { self } }
        impl<T: Measure> Measure for Box<T> { fn measure(self) -> int { self.contenido.measure() + 1 } }
        fn measure_dos<X: Measure>(a: X, b: X) -> int { a.measure() + b.measure() }
        fn main() -> int {
            let c = Box { contenido: 10 };
            measure_dos(c, c)   // (10+1) * 2 = 22
        }
    "#);
}

#[test]
fn impl_generic_deeply_nested_stress() {
    // Caja<Caja<int>>: un diccionario anidado que contiene otro. Bajo estrés del GC,
    // porque los closures-diccionario son objetos del heap (sus raíces deben trazarse).
    oracle_stress(r#"
        struct Box<T> { contenido: T }
        trait Measure { fn measure(self) -> int; }
        impl Measure for int { fn measure(self) -> int { self } }
        impl<T: Measure> Measure for Box<T> { fn measure(self) -> int { self.contenido.measure() + 1 } }
        fn measure_one<X: Measure>(x: X) -> int { x.measure() }
        fn main() -> int {
            let c2 = Box { contenido: Box { contenido: 100 } };
            c2.measure() + measure_one(c2)   // 102 + 102 = 204
        }
    "#);
}

// ----- M11.1: stdlib de string -----

#[test]
fn string_concat_len_oracle() {
    // Concatenación con `+`, len de string y to_string; el resultado es un int.
    oracle_program(r#"
        fn main() -> int {
            let s = "hello, " + "mundo";       // concat
            let label = "n=" + to_string(s.len());
            print(label);                   // n=11
            s.len() + "123".len()               // 11 + 3 = 14
        }
    "#);
}

#[test]
fn to_string_of_various_types_oracle() {
    oracle_program(r#"
        fn main() -> int {
            print(to_string(42));      // 42
            print(to_string(true));    // true
            print(to_string("ya"));    // ya (identidad)
            to_string(true).len() + to_string(false).len()   // 4 + 5 = 9
        }
    "#);
}

#[test]
fn string_ufcs_oracle() {
    // UFCS sobre los builtins de string (s.len(), n.to_string()).
    oracle_program(r#"
        fn main() -> int {
            let s = "raylang";
            print(s.len().to_string());   // 7
            s.len()
        }
    "#);
}

#[test]
fn string_trim_split_oracle() {
    oracle_program(r#"
        fn main() -> int {
            let clean = "  hello  ".trim();
            print("[" + clean + "]");        // [hola]
            let fields = "a,bb,ccc".split(",");
            print(fields[1]);                  // bb
            fields.len() + clean.len()          // 3 + 4 = 7
        }
    "#);
}

#[test]
fn char_ty_oracle() {
    // M11.4c-1: literal de char, anotación, ==, to_string, y @derive(Eq, Show) con campo char.
    oracle_program(r#"
        @derive(Eq, Show)
        struct Tecla { c: char, repeated: bool }
        fn class(c: char) -> int {
            if (c == 'a') { 1 } else { if (c == '\n') { 2 } else { 0 } }
        }
        fn main() -> int {
            let c: char = 'z';
            print(c);                              // z
            print(to_string('x') + "!");           // x!
            print('a' == 'a');                     // true
            let t = Tecla { c: 'q', repeated: false };
            print(t.show());                    // Tecla { c: q, repeated: false }
            print(t.eq(Tecla { c: 'q', repeated: false }));  // true
            class('a') + class('\n') + class('z')  // 1 + 2 + 0 = 3
        }
    "#);
}

#[test]
fn char_index_y_chars_oracle() {
    // M11.4c-2: s[i] -> char, chars(s) -> [char] (asigna heap → estrés del GC).
    oracle_stress(r#"
        fn account(s: string, c: char) -> int {
            var n = 0;
            var i = 0;
            while (i < s.len()) {
                if (s[i] == c) { n = n + 1; }
                i = i + 1;
            }
            n
        }
        fn main() -> int {
            let s = "racecar";
            print(s[0]);                       // r
            print(s[3]);                       // e
            let cs = s.chars();
            print(cs[1]);                      // a
            print(cs.len());                    // 7
            account(s, 'r') + account(s, 'c') + "hello".chars().len()  // 2 + 2 + 4 = 8
        }
    "#);
}

#[test]
fn string_contains_replace_oracle() {
    // contains -> bool; replace asigna un string nuevo (heap en la VM). Oráculo + estrés del GC.
    oracle_stress(r#"
        fn main() -> int {
            let s = "hello mundo, hello raylang";
            print(s.contains("mundo"));            // true
            print(s.contains("python"));           // false
            let r = s.replace("hello", "HOLA");
            print(r);                              // HOLA mundo, HOLA raylang
            print("a.b.c".replace(".", "/"));      // a/b/c
            if (s.contains("raylang")) { r.len() } else { 0 }  // 24
        }
    "#);
}

/// M43.1: **hashes de producción vía `ring`** (`sha256`/`sha512`/`sha1`). Doble red: el **oráculo**
/// (interp==vm) verifica CONSISTENCIA —ambos motores llaman al mismo `ring`—, y los **vectores conocidos**
/// (NIST/RFC) verifican CORRECCIÓN: el programa devuelve 1 solo si el hex calculado casa con el esperado,
/// así un error de corrección da 0 (que el oráculo por sí solo no detectaría si ambos motores fallaran
/// igual). Cubre entrada vacía y las tres funciones.
#[test]
fn sha_digests_oracle() {
    let cases = [
        ("__sha256", "abc", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        ("__sha256", "", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
        (
            "__sha512",
            "abc",
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
        ),
        ("__sha1", "abc", "a9993e364706816aba3e25717850c26c9cd0d89d"),
    ];
    for (f, input, hex) in cases {
        let src = format!(
            "fn main() -> int {{ if (to_string({f}(\"{input}\".to_bytes())) == \"{hex}\") {{ 1 }} else {{ 0 }} }}"
        );
        let tokens = crate::lexer::lex(&src).expect("lex ok");
        let mut prog = crate::parser::parse(tokens).expect("parse ok");
        crate::checker::check(&mut prog).expect("check ok");
        let interp = crate::interpreter::run(&prog).expect("interp ok");
        let compiled = compile_program(&prog).expect("compila");
        let vm = run_program(&compiled).expect("vm ok");
        assert_eq!(interp, vm, "VM≠intérprete en {f}(\"{input}\")");
        assert_eq!(vm, Value::Int(1), "{f}(\"{input}\") no casó con el vector conocido");
    }
    // Estrés del GC: cada hash asigna un `bytes` nuevo en el heap; encadenar hashes debe sobrevivir a
    // una recolección en cada paso seguro (destapa raíces faltantes).
    oracle_stress(
        r#"
        fn main() -> int {
            var acc = "seed".to_bytes();
            var i = 0;
            while (i < 50) {
                acc = __sha256(acc);       // 32 octetos, heap nuevo cada vuelta
                acc = __sha512(acc);       // 64 octetos
                acc = __sha1(acc);         // 20 octetos
                i = i + 1;
            }
            acc.len()                     // 20 (último es sha1)
        }
    "#,
    );
}

/// M43.2: **HMAC-SHA256** vía `ring`. Misma doble red: oráculo (interp==vm) + vector conocido
/// (RFC 4231, Test Case 2: clave `"Jefe"`, mensaje `"what do ya want for nothing?"`).
#[test]
fn hmac_sha256_oracle() {
    let src = format!(
        "fn main() -> int {{ if (to_string(__hmac_sha256(\"Jefe\".to_bytes(), \"{}\".to_bytes())) == \"{}\") {{ 1 }} else {{ 0 }} }}",
        "what do ya want for nothing?",
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    let tokens = crate::lexer::lex(&src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    crate::checker::check(&mut prog).expect("check ok");
    let interp = crate::interpreter::run(&prog).expect("interp ok");
    let compiled = compile_program(&prog).expect("compila");
    let vm = run_program(&compiled).expect("vm ok");
    assert_eq!(interp, vm, "VM≠intérprete en hmac_sha256");
    assert_eq!(vm, Value::Int(1), "hmac_sha256 no casó con el vector RFC 4231");
    // Estrés de GC: HMAC en cadena (clave y mensaje del paso previo).
    oracle_stress(
        r#"
        fn main() -> int {
            var k = "clave".to_bytes();
            var m = "mensaje".to_bytes();
            var i = 0;
            while (i < 50) {
                let t = __hmac_sha256(k, m);
                k = t;
                m = __sha256(t);
                i = i + 1;
            }
            k.len()                       // 32
        }
    "#,
    );
}

/// M43.3: **Ed25519** vía `ring` (`sign`/`verify`/`public_key`). Oráculo (interp==vm) + validación
/// RELACIONAL de corrección con `ring` como impl de confianza: la firma **verifica**, un mensaje
/// alterado **no**, la semilla corta da `None`, y firmar dos veces da lo mismo (determinismo, RFC 8032).
/// El programa devuelve 1 solo si TODO cuadra → un fallo de cableado da 0. La semilla son 32 octetos
/// ASCII (`to_bytes` de 32 chars) para no depender de literales de byte largos.
#[test]
fn ed25519_oracle() {
    // M49.3: la cripto pasó a `std/crypto`; el ORÁCULO (pre-loader) prueba los primitivos internos
    // `__ed25519_*` (arreglo etiquetado `[bytes]`: vacío = None), el envoltorio `crypto.ed25519_*`
    // (Option) lo cubren los tests de integración. Misma validación relacional con `ring`.
    let src = r#"
        fn main() -> int {
            let seed = "0123456789abcdef0123456789abcdef".to_bytes();   // 32 octetos
            let msg = "mensaje firmado".to_bytes();
            let pk_arr = __ed25519_public_key(seed);
            if (pk_arr.len() == 0) { 0 } else {
                let pk = pk_arr[0];
                let sig_arr = __ed25519_sign(seed, msg);
                if (sig_arr.len() == 0) { 0 } else {
                    let sig = sig_arr[0];
                    let ok = __ed25519_verify(pk, msg, sig);                       // true
                    let altered = __ed25519_verify(pk, "mensaje alterad".to_bytes(), sig); // false
                    let other = __ed25519_sign(seed, msg);                          // determinista
                    let det = other.len() > 0 && to_string(other[0]) == to_string(sig);
                    let corta = __ed25519_public_key("corta".to_bytes()).len() == 0; // no 32 → vacío
                    if (ok && !altered && det && corta) { 1 } else { 0 }
                }
            }
        }
    "#;
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    crate::checker::check(&mut prog).expect("check ok");
    let interp = crate::interpreter::run(&prog).expect("interp ok");
    let compiled = compile_program(&prog).expect("compila");
    let vm = run_program(&compiled).expect("vm ok");
    assert_eq!(interp, vm, "VM≠intérprete en ed25519");
    assert_eq!(vm, Value::Int(1), "Ed25519: falló roundtrip/manipulación/None/determinismo");
}

/// M43.4: **ChaCha20-Poly1305 AEAD** vía `ring`. Oráculo (interp==vm) + validación relacional:
/// `seal` luego `open` recupera el texto, alterar el `aad` hace fallar la autenticación (`None`), y una
/// clave de mal tamaño da `None` en `seal`. Devuelve 1 solo si todo cuadra.
#[test]
fn chacha20poly1305_oracle() {
    // M49.3: primitivos internos `__chacha20poly1305_*` (arreglo etiquetado `[bytes]`: vacío = None);
    // el envoltorio `crypto.*` (Option) lo cubren los tests de integración. Validación relacional.
    let src = r#"
        fn main() -> int {
            let key = "0123456789abcdef0123456789abcdef".to_bytes();   // 32 octetos
            let nonce = "nonce-de-12b".to_bytes();                     // 12 octetos
            let aad = "header".to_bytes();
            let pt = "text secreto".to_bytes();
            let ct_arr = __chacha20poly1305_seal(key, nonce, aad, pt);
            if (ct_arr.len() == 0) { 0 } else {
                let ct = ct_arr[0];
                let rec = __chacha20poly1305_open(key, nonce, aad, ct);
                let recovered = rec.len() > 0 && to_string(rec[0]) == to_string(pt);
                let tampered = __chacha20poly1305_open(key, nonce, "other cab".to_bytes(), ct).len() == 0;
                let corta = __chacha20poly1305_seal("corta".to_bytes(), nonce, aad, pt).len() == 0;
                if (recovered && tampered && corta) { 1 } else { 0 }
            }
        }
    "#;
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    crate::checker::check(&mut prog).expect("check ok");
    let interp = crate::interpreter::run(&prog).expect("interp ok");
    let compiled = compile_program(&prog).expect("compila");
    let vm = run_program(&compiled).expect("vm ok");
    assert_eq!(interp, vm, "VM≠intérprete en chacha20poly1305");
    assert_eq!(vm, Value::Int(1), "AEAD: falló roundtrip/autenticación/tamaño");
}

#[test]
fn string_stdlib_m117_oracle() {
    // M11.7a: starts_with/ends_with (bool); to_upper/to_lower/substring/repeat/join asignan
    // string nuevo (heap en la VM); index_of construye Option en el prelude. Oráculo + estrés GC.
    oracle_stress(r#"
        fn pos(o: Option<int>, def: int) -> int {
            match (o) { Option.Some(i) => i, Option.None => def, }
        }
        fn main() -> int {
            let s = "Hola, Mundo";
            print(s.starts_with("Hola"));      // true
            print(s.ends_with("xyz"));         // false
            print(s.to_upper());               // HOLA, MUNDO
            print(s.to_lower());               // hola, mundo
            print(s.substring(0, 4));          // Hola
            print(s.substring(6, 100));        // Mundo (clamp)
            print("ab".repeat(3));             // ababab
            print("".repeat(5));               // (vacío)
            let parts = ["a", "b", "c"];
            print(join(parts, "-"));          // a-b-c
            print(pos(index_of(s, "Mundo"), 0 - 1));   // 6
            print(pos(index_of(s, "zzz"), 0 - 1));      // -1
            s.substring(6, 11).len() + pos(index_of(s, "Mundo"), 0)  // 5 + 6 = 11
        }
    "#);
}

#[test]
fn array_stdlib_m117b_oracle() {
    // M11.7b: concat (a+b), reverse, pop (muta + Option), contains, position. reverse/pop/concat
    // asignan en el heap → estrés del GC; pop construye Option en el prelude.
    oracle_stress(r#"
        fn idx(o: Option<int>, def: int) -> int {
            match (o) { Option.Some(i) => i, Option.None => def, }
        }
        fn ult(o: Option<int>, def: int) -> int {
            match (o) { Option.Some(x) => x, Option.None => def, }
        }
        fn main() -> int {
            let a = [1, 2, 3];
            let b = [4, 5];
            let c = a + b;                      // [1,2,3,4,5]
            print(c.len());                      // 5
            let r = c.reverse();                 // [5,4,3,2,1]
            print(r[0]);                        // 5
            print(c.contains(4));               // true
            print(c.contains(99));              // false
            print(idx(position(c, 3), 0 - 1));  // 2
            print(idx(position(c, 99), 0 - 1)); // -1
            let v = [10, 20, 30];
            let x = ult(pop(v), 0);             // 30, y v queda [10,20]
            print(v.len());                      // 2
            x + c.len() + r[1]                   // 30 + 5 + 4 = 39
        }
    "#);
}

#[test]
fn sort_ord_oracle() {
    // M11.7d: sort<T: Ord> (bound → diccionarios M9.2) sobre primitivos y un tipo de usuario
    // que implementa Ord. Asigna arreglos en el heap → estrés del GC.
    oracle_stress(r#"
        struct Box { peso: int }
        impl Ord for Box {
            fn less(self, other: Box) -> bool { self.peso < other.peso }
        }
        fn main() -> int {
            let xs = sort([3, 1, 4, 1, 5, 9, 2, 6]);
            print(xs[0]); print(xs[7]);             // 1 ... 9
            let cs = sort(['c', 'a', 'b']);
            print(cs[0]);                            // a
            let boxes = sort([Box { peso: 30 }, Box { peso: 10 }, Box { peso: 20 }]);
            print(boxes[0].peso);                    // 10
            print(boxes[2].peso);                    // 30
            xs[0] + xs[7] + boxes[0].peso            // 1 + 9 + 10 = 20
        }
    "#);
}

#[test]
fn string_split_stress_gc() {
    // split asigna un arreglo (objeto del heap). Bajo estrés del GC: si una raíz faltara,
    // el arreglo recién creado se liberaría y el resultado cambiaría.
    oracle_stress(r#"
        fn main() -> int {
            let parts = "one:dos:tres:cuatro".trim().split(":");
            let total = parts.len() + parts[0].len() + parts[3].len();
            print(parts[2]);                  // tres
            total                              // 4 + 3 + 6 = 13
        }
    "#);
}

#[test]
fn parse_int_oracle() {
    // parse_int es determinista (no toca stdin) → oráculo VM↔intérprete. Construye Option
    // en el prelude (raylang); el resultado debe coincidir en ambos motores.
    oracle_program(r#"
        fn value_or(o: Option<int>, def: int) -> int {
            match (o) {
                Option.Some(n) => n,
                Option.None => def,
            }
        }
        fn main() -> int {
            let a = value_or(parse_int("42"), 0);        // 42
            let b = value_or(parse_int("  -7 "), 0);     // -7 (trim)
            let c = value_or(parse_int("xyz"), 100);     // 100 (None)
            a + b + c                                 // 135
        }
    "#);
}

#[test]
fn print_uint_oracle() {
    // jul 2026: print/eprint/to_string aceptan u8/u32/u64 (decimal sin signo, mismo Display en
    // ambos motores). Incluye la interpolación (desazucara a to_string) y el borde de u64 alto.
    oracle_program(r#"
        fn main() -> int {
            let a: u8 = 200;
            let b: u32 = 4294967295;
            var c: u64 = 0;
            c = c - 1; // wrapping → u64::MAX (el literal no cabe en el lexer, que vive en i64)
            print(a);
            print(b);
            print(c);
            let s = "${a}-${b}";
            print(s);
            if (to_string(a) == "200" && s == "200-4294967295") { 7 } else { -1 }
        }
    "#);
}

#[test]
fn char_from_code_oracle() {
    // Diferido JSON-1: char_from_code es el char::from_u32 de Rust en ambos motores. Válidos
    // (ASCII, multi-byte, astral) e inválidos (surrogate, fuera de rango, negativo, enorme —
    // este último caza un wrap del cast a u32).
    oracle_program(r#"
        fn main() -> int {
            let a: int = match (char_from_code(65)) { Option.Some(c) => if (c == 'A') { 1 } else { -1 }, Option.None => -1 };
            let e: int = match (char_from_code(233)) { Option.Some(c) => if (to_string(c) == "é") { 10 } else { -1 }, Option.None => -1 };
            let astral: int = match (char_from_code(128512)) { Option.Some(c) => if (char_code(c) == 128512) { 100 } else { -1 }, Option.None => -1 };
            let sur: int = match (char_from_code(55296)) { Option.Some(_) => -1, Option.None => 1000 };
            let outside: int = match (char_from_code(1114112)) { Option.Some(_) => -1, Option.None => 10000 };
            let neg: int = match (char_from_code(0 - 1)) { Option.Some(_) => -1, Option.None => 100000 };
            let wrap: int = match (char_from_code(4294967361)) { Option.Some(_) => -1, Option.None => 1000000 };
            a + e + astral + sur + outside + neg + wrap
        }
    "#);
}

#[test]
fn float_bits_oracle() {
    // M54.1: __float_bits/__float_from_bits son el f64 de Rust en ambos motores → oráculo.
    // Round-trip exacto (incl. negativos y el caso 5.05 del vector BSON) y bits conocidos:
    // 1.0 = 0x3FF0000000000000 = 4607182418800017408.
    oracle_program(r#"
        fn main() -> int {
            let one = __float_bits(1.0);
            let a: int = if (one == 4607182418800017408) { 1 } else { -1 };
            let b: int = if (__float_from_bits(one) == 1.0) { 10 } else { -1 };
            let c: int = if (__float_from_bits(__float_bits(5.05)) == 5.05) { 100 } else { -1 };
            let d: int = if (__float_from_bits(__float_bits(0.0 - 2.5)) == 0.0 - 2.5) { 1000 } else { -1 };
            a + b + c + d
        }
    "#);
}

#[test]
fn parse_float_oracle() {
    // M14: parse_float, como parse_int, es determinista → oráculo. El formateo de float es
    // el mismo f64 de Rust en ambos motores, así que los valores coinciden.
    oracle_program(r#"
        fn main() -> int {
            let ok = match (parse_float("3.14")) { Option.Some(f) => f, Option.None => 0.0 };
            let no = match (parse_float("hello")) { Option.Some(_) => 1, Option.None => 0 };
            let ent = match (parse_float("42")) { Option.Some(f) => f, Option.None => 0.0 };
            // 3.14*100 = 314, 42.0 → 42; no=0. Resultado 314 + 42 + 0 = 356.
            let a: int = if (ok * 100.0 == 314.0) { 314 } else { -1 };
            let b: int = if (ent == 42.0) { 42 } else { -1 };
            a + b + no
        }
    "#);
}

#[test]
fn args_y_env_oracle() {
    // En el proceso de test no se fijan args (→ []) y la variable no existe (→ None): ambos
    // motores deben coincidir. (El comportamiento "real" se prueba por subproceso en io_cli.)
    oracle_program(r#"
        fn main() -> int {
            let n = args().len();                       // 0
            let e = match (env("RAYLANG_NO_EXISTE_XYZ")) {
                Option.Some(_) => 1,
                Option.None => 0,
            };
            n + e                                      // 0
        }
    "#);
}

#[test]
fn read_file_nonexistent_is_err_oracle() {
    // Leer un archivo inexistente es determinista (misma llamada a std::fs en ambos motores) →
    // oráculo. El oráculo es pre-loader (M50.1: read_file vive en std/fs), así que usa el
    // primitivo __read_file directamente (arreglo etiquetado ["ok",…]/["err",msg]); debe coincidir.
    oracle_program(r#"
        fn main() -> int {
            let r = __read_file("/raylang_no_existe_xyz_123.txt");
            if (r[0] == "ok") { 0 } else { 1 }
        }
    "#);
}

#[test]
fn parse_int_option_construido_en_el_heap_stress_gc() {
    // El [int] del primitivo y el Option que arma el prelude son objetos del heap. Bajo
    // estrés del GC: si una raíz faltara, el valor vivo se liberaría.
    oracle_stress(r#"
        fn main() -> int {
            let xs = ["1", "2", "no", "4"];
            var sum = 0;
            var i = 0;
            while (i < xs.len()) {
                match (parse_int(xs[i])) {
                    Option.Some(n) => { sum = sum + n; },
                    Option.None => {},
                }
                i = i + 1;
            }
            sum                               // 1 + 2 + 4 = 7
        }
    "#);
}

// ----- M9.3a: métodos por defecto -----

#[test]
fn default_methods_oracle() {
    // Defecto heredado, defecto que llama a otro método, y redefinición. El método
    // sintetizado es una función ordinaria: ambos motores deben coincidir.
    oracle_program(r#"
        trait Value {
            fn base(self) -> int;
            fn double(self) -> int { self.base() + self.base() }   // defecto usa otro
            fn ten(self) -> int { 10 }                            // defecto constante
        }
        struct A { n: int }
        impl Value for A { fn base(self) -> int { self.n } }       // hereda doble y diez
        struct B { n: int }
        impl Value for B {
            fn base(self) -> int { self.n }
            fn double(self) -> int { self.n * 100 }                 // redefine doble
        }
        fn main() -> int {
            let a = A { n: 3 };
            let b = B { n: 4 };
            a.double() + a.ten() + b.double() + b.ten()   // 6 + 10 + 400 + 10 = 426
        }
    "#);
}

#[test]
fn default_methods_via_bound_oracle() {
    // Un método por defecto invocado desde un genérico acotado (M9.2 + M9.3a).
    oracle_stress(r#"
        trait Greeting {
            fn name(self) -> int;
            fn double_name(self) -> int { self.name() + self.name() }
        }
        struct P { v: int }
        impl Greeting for P { fn name(self) -> int { self.v } }
        fn usar<T: Greeting>(x: T) -> int { x.double_name() }
        fn main() -> int { let p = P { v: 21 }; usar(p) }   // 42
    "#);
}

// ----- M9.3b: trait objects (despacho dinámico) -----

#[test]
fn trait_objects_dispatch_dynamic_oracle() {
    // Arreglo heterogéneo de trait objects + despacho por valor. El trait object se
    // realiza como un struct sintetizado (la vtable); ambos motores deben coincidir.
    oracle_program(r#"
        trait Shape { fn area(self) -> int; }
        struct Square { lado: int }
        impl Shape for Square { fn area(self) -> int { self.lado * self.lado } }
        struct Rect { ancho: int, alto: int }
        impl Shape for Rect { fn area(self) -> int { self.ancho * self.alto } }
        fn total(xs: [dyn Shape]) -> int {
            var s = 0; var i = 0;
            while (i < xs.len()) { s = s + xs[i].area(); i = i + 1; }
            s
        }
        fn main() -> int {
            let shapes: [dyn Shape] = [Square{lado:3}, Rect{ancho:4,alto:5}, Square{lado:2}];
            total(shapes)   // 9 + 20 + 4 = 33
        }
    "#);
}

#[test]
fn dyn_multi_trait_oracle() {
    // M9.5a: `dyn A + B` — un objeto que satisface dos traits; despacho a métodos de ambos.
    // El orden del conjunto es canónico (dyn Nombre + Area == dyn Area + Nombre).
    oracle_program(r#"
        trait Area { fn area(self) -> int; }
        trait Name { fn name(self) -> string; }
        struct Square { lado: int }
        impl Area for Square { fn area(self) -> int { self.lado * self.lado } }
        impl Name for Square { fn name(self) -> string { "cuad" } }
        struct Circ { r: int }
        impl Area for Circ { fn area(self) -> int { 3 * self.r * self.r } }
        impl Name for Circ { fn name(self) -> string { "circ" } }
        fn describe(x: dyn Name + Area) -> int { x.name().len() + x.area() }
        fn main() -> int {
            let xs: [dyn Area + Name] = [Square{lado:4}, Circ{r:2}];
            var s = 0; var i = 0;
            while (i < xs.len()) { s = s + describe(xs[i]); i = i + 1; }
            // (4 + 16) + (4 + 12) = 20 + 16 = 36
            s
        }
    "#);
}

#[test]
fn dyn_upcasting_oracle() {
    // M9.5b: upcasting `dyn A + B` -> `dyn A` (olvidar traits, S2 ⊆ S1). Se reconstruye el
    // struct menor proyectando los campos del mayor.
    oracle_program(r#"
        trait Area { fn area(self) -> int; }
        trait Name { fn name(self) -> string; }
        struct Square { lado: int }
        impl Area for Square { fn area(self) -> int { self.lado * self.lado } }
        impl Name for Square { fn name(self) -> string { "cuad" } }
        fn solo_area(a: dyn Area) -> int { a.area() }
        fn main() -> int {
            let ab: dyn Area + Name = Square { lado: 5 };
            let v1 = solo_area(ab);        // upcast en el argumento: 25
            let a: dyn Area = ab;          // upcast en el let
            v1 + a.area()                  // 25 + 25 = 50
        }
    "#);
}

#[test]
fn dyn_about_impl_generic_oracle() {
    // M9.4: coercionar a `dyn Trait` un tipo cuyo impl es genérico acotado (Caja<T>): la vtable
    // lleva un closure anidado (como un diccionario), no el método manglado plano. Incluye
    // anidamiento Caja<Caja<N>> y un impl concreto en el mismo arreglo heterogéneo.
    oracle_program(r#"
        trait Show { fn show(self) -> string; }
        struct N { x: int }
        impl Show for N { fn show(self) -> string { "N" } }
        struct Box<T> { v: T }
        impl<T: Show> Show for Box<T> {
            fn show(self) -> string { "Box(" + self.v.show() + ")" }
        }
        fn describe(d: dyn Show) -> string { d.show() }
        fn main() -> int {
            let xs: [dyn Show] = [N{x:1}, Box{v:N{x:2}}, Box{v:Box{v:N{x:3}}}];
            var total = 0; var i = 0;
            while (i < xs.len()) { total = total + describe(xs[i]).len(); i = i + 1; }
            // len("N")=1, len("Box(N)")=6, len("Box(Box(N))")=11 -> 18
            total
        }
    "#);
}

#[test]
fn default_with_self_inherited_by_two_impls() {
    // Regresión: un método por defecto que llama a `self.m()` y es heredado por DOS
    // impls. Cada cuerpo clonado debe resolver a SUS métodos (no compartir destino).
    oracle_program(r#"
        trait Animal {
            fn sound(self) -> int;
            fn double_sound(self) -> int { self.sound() + self.sound() }   // defecto
        }
        struct Dog { v: int }
        impl Animal for Dog { fn sound(self) -> int { self.v } }            // hereda
        struct Cat { v: int }
        impl Animal for Cat { fn sound(self) -> int { self.v * 10 } }        // hereda
        fn main() -> int {
            let p = Dog { v: 3 };
            let g = Cat { v: 4 };
            p.double_sound() + g.double_sound()   // (3+3) + (40+40) = 6 + 80 = 86
        }
    "#);
}

#[test]
fn trait_objects_stress_gc() {
    // El struct sintetizado (vtable) y el dato viven en el heap de la VM: el GC debe
    // trazar ambos. Bajo estrés (recolecta en cada punto seguro), un fallo de raíz
    // cambiaría el resultado o reventaría.
    oracle_stress(r#"
        trait Value { fn value(self) -> int; fn double(self) -> int { self.value() + self.value() } }
        struct A { n: int }
        impl Value for A { fn value(self) -> int { self.n } }
        struct B { n: int }
        impl Value for B { fn value(self) -> int { self.n + 1 } fn double(self) -> int { self.n } }
        fn usar(x: dyn Value) -> int { x.value() + x.double() }
        fn main() -> int {
            let a: dyn Value = A { n: 10 };
            let b: dyn Value = B { n: 20 };
            usar(a) + usar(b)   // (10+20) + (21+20) = 30 + 41 = 71
        }
    "#);
}

// ----- M10.1: @derive(Eq) -----

#[test]
fn derive_eq_oracle() {
    // El impl generado por @derive(Eq) baja a una función ordinaria (M9): ambos motores
    // deben coincidir, para struct, enum unit y enum con payload.
    oracle_program(r#"
        @derive(Eq)
        struct Point { x: int, y: int }
        @derive(Eq)
        enum Color { Rojo, Verde, Azul }
        @derive(Eq)
        enum Form { Circulo(int), Rect(int, int) }
        fn b2i(b: bool) -> int { if (b) { 1 } else { 0 } }
        fn main() -> int {
            let p = Point { x: 1, y: 2 };
            let q = Point { x: 1, y: 2 };
            let r = Point { x: 9, y: 2 };
            let e1 = b2i(p.eq(q)) + b2i(p.eq(r));               // 1 + 0
            let e2 = b2i(Color.Verde.eq(Color.Verde)) + b2i(Color.Rojo.eq(Color.Azul)); // 1 + 0
            let f = Form.Rect(3, 4);
            let e3 = b2i(f.eq(Form.Rect(3, 4))) + b2i(f.eq(Form.Circulo(3)));         // 1 + 0
            e1 + e2 + e3   // 3
        }
    "#);
}

/// V2 (bench políglota): la bajada `lower_concat` reescribe las cadenas de `+` de strings (y la
/// interpolación, que desazucara a `+`) al primitivo `__concat` → opcode `ConcatN`. Este oráculo
/// fija que la reescritura preserva la semántica en ambos motores: cadenas largas, interpolación,
/// operandos que son a su vez cadenas anidadas en llamadas, y la mezcla con `+` no-string (int).
#[test]
fn concat_chain_lowering_oracle() {
    oracle_program(
        r#"
        fn wrap(s: string) -> string { "[" + s + "]" }
        fn main() -> int {
            var acc = 0;
            var i = 0;
            while (i < 100) {
                let a = "id:" + to_string(i) + ",n:" + wrap("u" + to_string(i * 2)) + "!";
                let b = "x${i}y${i % 7}z";
                acc = acc + a.len() + b.len();
                i = i + 1;
            }
            print(acc);
            acc
        }
        "#,
    );
}

/// V5 (bench políglota): `lower_sort_prim` reescribe el `sort` del prelude sobre primitivos a
/// `__sort_prim` (sort nativo). Este oráculo fija que la reescritura preserva la semántica y que
/// los caminos NO reescritos siguen intactos: float (excluido por NaN), tipo de usuario con
/// `impl Ord` (diccionario), y un `sort` REDEFINIDO por el usuario (override → sin reescritura).
#[test]
fn sort_prim_lowering_oracle() {
    // Primitivos (reescritos) + float y tipo de usuario (camino del prelude).
    oracle_program(
        r#"
        struct P { n: int }
        impl Ord for P { fn less(self, other: Self) -> bool { self.n > other.n } }
        fn main() -> int {
            let s = ["pera", "uva", "kiwi", "uva", "anis"].sort();
            let i = [5, 1, 4, 1, 3].sort();
            let c = ['z', 'a', 'm'].sort();
            let f = [2.5, 0.5, 1.5].sort();
            let p = [P { n: 1 }, P { n: 9 }, P { n: 4 }].sort();
            print(s.join(","));
            print(i[0] + i[4]);
            print(c[0]);
            print(f[0]);
            print(p[0].n);
            0
        }
        "#,
    );
    // Override del usuario: su `sort` (descendente) NO debe reescribirse a `__sort_prim`.
    oracle_program(
        r#"
        fn sort(a: [int]) -> [int] {
            var out: [int] = [];
            var i = 0;
            while (i < a.len()) { out.push(a[i]); i = i + 1; }
            var j = 0;
            while (j < out.len()) {
                var k = j + 1;
                while (k < out.len()) {
                    if (out[k] > out[j]) { let t = out[j]; out[j] = out[k]; out[k] = t; }
                    k = k + 1;
                }
                j = j + 1;
            }
            out
        }
        fn main() -> int {
            let d = sort([1, 9, 5]);
            print(d[0]);
            d[0] - 9
        }
        "#,
    );
}

/// D3 (jsondeserialize): `lower_prelude_fusions` reescribe `index_of(…)/parse_int(…) .unwrap_or(d)`
/// a `__index_of_or`/`__parse_int_or`. Fija: los casos fusionados (hallado/no hallado, parseable/no,
/// no-ASCII), los NO fusionados (unwrap_or sobre otros Option; el wrapper sin unwrap_or), y el
/// override del usuario de `index_of` (no debe fusionarse: su semántica es otra).
#[test]
fn option_unwrap_or_fusion_oracle() {
    oracle_program(
        r#"
        fn main() -> int {
            let s = "{\"id\":42,\"name\":\"ana\"}";
            let a = s.index_of(",\"name\"").unwrap_or(0 - 1);
            let b = s.index_of("zzz").unwrap_or(0 - 1);
            let c = parse_int("  77 ").unwrap_or(0);
            let d = parse_int("nope").unwrap_or(0 - 5);
            let e = "añô€x".index_of("€x").unwrap_or(0 - 1);
            let f = s.index_of("42");
            let g = match (f) { Option.Some(i) => i, Option.None => 0 - 1, };
            let h = [1, 2].pop().unwrap_or(0 - 9);
            print(a); print(b); print(c); print(d); print(e); print(g); print(h);
            0
        }
        "#,
    );
    // Override del usuario: su `index_of` (semántica distinta: siempre Some(len)) NO se fusiona.
    oracle_program(
        r#"
        fn index_of(s: string, sub: string) -> Option<int> { Option.Some(s.len()) }
        fn main() -> int {
            let x = "hola".index_of("zzz").unwrap_or(0 - 1);
            print(x);
            x - 4
        }
        "#,
    );
}

/// M100 fase 1a-bis: el builtin `__run` (procesos del SO) da lo MISMO en los dos motores. El score
/// en bits detecta qué campo del arreglo etiquetado divergió; además se asevera el valor esperado
/// (no solo la paridad) para que un fallo compartido de ambos motores no pase en silencio.
#[cfg(unix)]
#[test]
fn run_builtin_oracle_and_expected_value() {
    let src = r#"
        fn main() -> int {
            let none: [string] = [];
            let r = __run("sh", ["-c", "printf abc; printf de >&2; exit 7"], "", none, false,
                          "".to_bytes(), false, 0, 1048576, false);
            var score = 0;
            if (r[0] == "ok".to_bytes()) { score = score + 1; }
            if (r[1] == "code".to_bytes()) { score = score + 2; }
            if (r[2] == "7".to_bytes()) { score = score + 4; }
            if (r[3] == "0".to_bytes()) { score = score + 8; }
            if (r[4] == "0".to_bytes()) { score = score + 16; }
            if (r[5] == "abc".to_bytes()) { score = score + 32; }
            if (r[6] == "de".to_bytes()) { score = score + 64; }
            // stdin: se escribe y se cierra; `cat` lo devuelve entero.
            let c = __run("cat", none, "", none, false, "hola".to_bytes(), true, 0, 1048576, false);
            if (c[5] == "hola".to_bytes()) { score = score + 128; }
            score
        }
    "#;
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    crate::checker::check(&mut prog).expect("check ok");
    let interp = crate::interpreter::run(&prog).expect("intérprete ok");
    let compiled = compile_program(&prog).expect("compila");
    let vm = run_program(&compiled).expect("vm ok");
    assert_eq!(interp, Value::Int(255), "intérprete: campos divergentes (bits apagados)");
    assert_eq!(vm, Value::Int(255), "VM: campos divergentes (bits apagados)");
}

/// M100 v2 fase 2a: los primitivos del streaming (__proc_spawn/__proc_try_wait/__proc_kill) y la
/// lectura de un handle Pipe por __socket_read_bytes (que APARCA la fibra en WouldBlock — desde
/// raylang una lectura simplemente bloquea hasta datos o EOF). Sin spawn/canales: el oráculo
/// intérprete≡VM aplica; las bombas de verdad son de la fase 2b (std/process).
#[cfg(unix)]
#[test]
fn proc_streaming_primitives_oracle() {
    let src = r#"
        fn handle_of(b: bytes) -> int {
            match (from_utf8(b)) {
                Result.Ok(s) => parse_int(s).unwrap_or(0 - 1),
                Result.Err(e) => 0 - 1,
            }
        }
        fn main() -> int {
            let none: [string] = [];
            let r = __proc_spawn("sh", ["-c", "printf abc; printf de >&2; exit 5"], "", none,
                                 false, "".to_bytes(), false, false);
            var score = 0;
            if (r[0] == "ok".to_bytes()) { score = score + 1; }
            let h_child = handle_of(r[1]);
            let h_out = handle_of(r[2]);
            let h_err = handle_of(r[3]);
            if (h_child > 0 && h_out > 0 && h_err > 0) { score = score + 2; }
            // Lectura del pipe: la primera da los datos (aparca hasta que lleguen), la segunda EOF.
            let o1 = __socket_read_bytes(h_out);
            if (o1[0] == "ok".to_bytes() && o1[1] == "abc".to_bytes()) { score = score + 4; }
            let o2 = __socket_read_bytes(h_out);
            if (o2[0] == "ok".to_bytes() && o2[1].len() == 0) { score = score + 8; }
            let e1 = __socket_read_bytes(h_err);
            if (e1[0] == "ok".to_bytes() && e1[1] == "de".to_bytes()) { score = score + 16; }
            close(h_out);
            close(h_err);
            // Tras el EOF el hijo ya salió (o está a un tick): el try_wait cosecha en breve.
            var tag = "".to_bytes();
            var val = "".to_bytes();
            var waiting = true;
            while (waiting) {
                let w = __proc_try_wait(h_child);
                if (w[0] == "running".to_bytes()) { } else {
                    tag = w[0];
                    if (w.len() > 1) { val = w[1]; }
                    waiting = false;
                }
            }
            if (tag == "code".to_bytes() && val == "5".to_bytes()) { score = score + 32; }
            // El handle cosechado se ELIMINA: un segundo try_wait es err, y kill es no-op.
            let w2 = __proc_try_wait(h_child);
            if (w2[0] == "err".to_bytes()) { score = score + 64; }
            __proc_kill(h_child, false);
            // Kill de verdad: al GRUPO, y el estado sale como signal.
            let k = __proc_spawn("sleep", ["30"], "", none, false, "".to_bytes(), false, false);
            let hk = handle_of(k[1]);
            __proc_kill(hk, false);
            var ktag = "".to_bytes();
            var kval = "".to_bytes();
            var kwait = true;
            while (kwait) {
                let w = __proc_try_wait(hk);
                if (w[0] == "running".to_bytes()) { } else {
                    ktag = w[0];
                    if (w.len() > 1) { kval = w[1]; }
                    kwait = false;
                }
            }
            if (ktag == "signal".to_bytes() && kval == "15".to_bytes()) { score = score + 128; }
            score
        }
    "#;
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    crate::checker::check(&mut prog).expect("check ok");
    let interp = crate::interpreter::run(&prog).expect("intérprete ok");
    let compiled = compile_program(&prog).expect("compila");
    let vm = run_program(&compiled).expect("vm ok");
    assert_eq!(interp, Value::Int(255), "intérprete: campos divergentes (bits apagados)");
    assert_eq!(vm, Value::Int(255), "VM: campos divergentes (bits apagados)");
}

// ── MM4: kernel DotRange (producto punto con deopt) ──────────────────────────

/// Oráculo de ERROR a nivel de programa: ambos motores fallan con el mismo mensaje y en la
/// misma LÍNEA. (La columna puede diferir en un indexado fusionado: `IndexLL` registra la
/// posición del primer opcode del par — divergencia PREEXISTENTE de las superinstrucciones,
/// verificada también sin kernel; no la introduce MM4.)
fn oracle_error(src: &str) {
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    crate::checker::check(&mut prog).expect("check ok");
    let interp = crate::interpreter::run(&prog).expect_err("el intérprete debe fallar");
    let compiled = compile_program(&prog).expect("compila");
    let vm = run_program(&compiled).expect_err("la VM debe fallar");
    assert_eq!(
        (interp.msg.as_str(), interp.line),
        (vm.msg.as_str(), vm.line),
        "intérprete y VM difieren en el error"
    );
}

/// ¿El bytecode de alguna función del programa contiene un `DotRange`?
fn has_dot_range(src: &str) -> bool {
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    crate::checker::check(&mut prog).expect("check ok");
    let compiled = compile_program(&prog).expect("compila");
    compiled.functions.iter().any(|f| {
        f.chunk.code.iter().any(|op| matches!(op, OpCode::DotRange { .. }))
    })
}

/// El camino rápido: arreglos de floats, resultado bit a bit idéntico al intérprete.
#[test]
fn dot_range_kernel_floats_oracle() {
    let src = r#"
        fn main() -> int {
            var a: [float] = [];
            var b: [float] = [];
            for i in 0..97 {
                a.push(((i * 7) % 13) as float);
                b.push(((i * 11) % 17) as float);
            }
            var s = 0.0;
            for k in 0..97 {
                s = s + a[k] * b[k];
            }
            (s * 1000.0) as int
        }
    "#;
    assert!(has_dot_range(src), "el patrón exacto emite el kernel");
    oracle_program(src);
}

/// dot(a, a) — el mismo arreglo en ambos lados es válido (nada muta dentro del bucle).
#[test]
fn dot_range_kernel_same_array_oracle() {
    oracle_program(
        r#"
        fn main() -> int {
            var a: [float] = [];
            for i in 0..31 { a.push((i % 7) as float); }
            var s = 0.0;
            for k in 0..31 { s = s + a[k] * a[k]; }
            s as int
        }
    "#,
    );
}

/// Rango vacío (lo >= hi): el kernel no toca nada, como el bucle normal.
#[test]
fn dot_range_kernel_empty_range_oracle() {
    oracle_program(
        r#"
        fn main() -> int {
            var a: [float] = [];
            var b: [float] = [];
            a.push(1.0);
            b.push(2.0);
            var s = 10.0;
            for k in 5..3 { s = s + a[k] * b[k]; }
            s as int
        }
    "#,
    );
}

/// DEOPT por representación: arreglos de INTS (acc int) — cae al bucle interpretado
/// y el resultado (con la aritmética checked de int) coincide con el intérprete.
#[test]
fn dot_range_deopt_int_arrays_oracle() {
    let src = r#"
        fn main() -> int {
            var a: [int] = [];
            var b: [int] = [];
            for i in 0..50 { a.push(i % 9); b.push(i % 5); }
            var s = 0;
            for k in 0..50 { s = s + a[k] * b[k]; }
            s
        }
    "#;
    assert!(has_dot_range(src), "el kernel se emite (la forma es la misma); en runtime deopta");
    oracle_program(src);
}

/// DEOPT por rango: el arreglo es más corto que el rango → el error de índice nace en el
/// bucle NORMAL, con el mismo mensaje y la misma posición que el intérprete.
#[test]
fn dot_range_deopt_out_of_bounds_error_parity() {
    oracle_error(
        r#"
        fn main() -> int {
            var a: [float] = [];
            var b: [float] = [];
            for i in 0..10 { a.push(i as float); b.push(i as float); }
            var s = 0.0;
            for k in 0..20 { s = s + a[k] * b[k]; }
            s as int
        }
    "#,
    );
}

/// DEOPT por overflow de int: el trap (con su posición) lo da el bucle normal.
#[test]
fn dot_range_deopt_int_overflow_error_parity() {
    oracle_error(
        r#"
        fn main() -> int {
            var a: [int] = [];
            var b: [int] = [];
            a.push(4611686018427387904);
            b.push(4);
            var s = 0;
            for k in 0..1 { s = s + a[k] * b[k]; }
            s
        }
    "#,
    );
}

/// DEOPT por acumulador BOXEADO (capturado por una closure): el kernel exige Plain.
#[test]
fn dot_range_deopt_boxed_acc_oracle() {
    oracle_program(
        r#"
        fn main() -> int {
            var a: [float] = [];
            var b: [float] = [];
            for i in 0..20 { a.push((i % 4) as float); b.push((i % 3) as float); }
            var s = 0.0;
            let lee = fn() -> float { s };
            for k in 0..20 { s = s + a[k] * b[k]; }
            (s + lee() * 0.0) as int
        }
    "#,
    );
}

/// Los NO-patrones no emiten el kernel: índices distintos, otro operador, acumulador aliado.
#[test]
fn dot_range_not_emitted_for_non_patterns() {
    // b[j] con j fijo: no es el índice del rango.
    assert!(!has_dot_range(
        r#"
        fn main() -> int {
            var a: [float] = []; var b: [float] = []; a.push(1.0); b.push(1.0);
            let j = 0;
            var s = 0.0;
            for k in 0..1 { s = s + a[k] * b[j]; }
            s as int
        }
    "#
    ));
    // Resta en vez de suma.
    assert!(!has_dot_range(
        r#"
        fn main() -> int {
            var a: [float] = []; var b: [float] = []; a.push(1.0); b.push(1.0);
            var s = 0.0;
            for k in 0..1 { s = s - a[k] * b[k]; }
            s as int
        }
    "#
    ));
    // El acumulador es uno de los arreglos... (tipos no lo permiten con floats; la
    // guarda sintáctica s != a se cubre con el índice como acumulador imposible de
    // tipar — basta con verificar que el patrón con cuerpo extra tampoco fusiona):
    assert!(!has_dot_range(
        r#"
        fn main() -> int {
            var a: [float] = []; var b: [float] = []; a.push(1.0); b.push(1.0);
            var s = 0.0;
            var t = 0.0;
            for k in 0..1 { s = s + a[k] * b[k]; t = t + 1.0; }
            (s + t) as int
        }
    "#
    ));
}

// ── V9: ronda 5 de superinstrucciones (guarda local-local y cierre de bucle) ──

/// ¿El bytecode de alguna función contiene el opcode indicado (por matcher)?
fn has_op(src: &str, pred: fn(&OpCode) -> bool) -> bool {
    let tokens = crate::lexer::lex(src).expect("lex ok");
    let mut prog = crate::parser::parse(tokens).expect("parse ok");
    crate::checker::check(&mut prog).expect("check ok");
    let compiled = compile_program(&prog).expect("compila");
    compiled.functions.iter().any(|f| f.chunk.code.iter().any(|op| pred(op)))
}

/// El for-range con tope en VARIABLE fusiona la guarda (LocalLocalCmpJump) y el cierre
/// (IncJump); el resultado coincide con el intérprete.
#[test]
fn round5_counted_loop_oracle() {
    let src = r#"
        fn main() -> int {
            let n = 1000;
            var s = 0;
            for i in 0..n { s = (s + i * i) % 97; }
            s
        }
    "#;
    assert!(has_op(src, |op| matches!(op, OpCode::LocalLocalCmpJump(..))), "guarda fusionada");
    assert!(has_op(src, |op| matches!(op, OpCode::IncJump(..))), "cierre fusionado");
    oracle_program(src);
}

/// El incremento manual de un `while` (sin salto justo detrás por el cuerpo) fusiona a
/// IncLocalConst; mismo valor que el intérprete.
#[test]
fn round5_manual_increment_oracle() {
    oracle_program(
        r#"
        fn main() -> int {
            var i = 0;
            var s = 0;
            while (i < 500) {
                s = s + i;
                i = i + 3;
            }
            s + i
        }
    "#,
    );
}

/// El overflow del incremento fusionado conserva el error (mensaje y línea) del intérprete.
#[test]
fn round5_increment_overflow_error_parity() {
    oracle_error(
        r#"
        fn main() -> int {
            var i = 9223372036854775800;
            var s = 0;
            while (s < 3) {
                s = s + 1;
                i = i + 100;
            }
            s
        }
    "#,
    );
}

/// Un contador CAPTURADO por una closure va boxeado: IncLocalConst/IncJump escriben vía la
/// celda (set_local) y la closure observa el valor final correcto.
#[test]
fn round5_boxed_counter_oracle() {
    oracle_program(
        r#"
        fn main() -> int {
            var i = 0;
            let lee = fn() -> int { i };
            var s = 0;
            while (i < 100) {
                s = s + 2;
                i = i + 1;
            }
            s + lee()
        }
    "#,
    );
}

/// Guarda local-local con tipos NO enteros (floats): cae al fallback de apply_binary y
/// coincide con el intérprete.
#[test]
fn round5_float_guard_oracle() {
    oracle_program(
        r#"
        fn main() -> int {
            var x = 0.0;
            let tope = 10.5;
            var vueltas = 0;
            while (x < tope) {
                x = x + 1.25;
                vueltas = vueltas + 1;
            }
            vueltas
        }
    "#,
    );
}
