//! Registro único de los **builtins** del lenguaje (limpieza post-M11, L1).
//!
//! Antes, cada builtin (`print`, `len`, `split`, `args`, …) se repetía en ~4 sitios: el checker
//! (membresía + regla de tipos), el intérprete (despacho) y el compilador (nombre → opcode). Añadir
//! uno obligaba a tocarlos todos y era fácil desincronizarlos. Aquí viven, en **una sola tabla**:
//!
//! - el **nombre** con que se invocan,
//! - el **opcode** que los implementa en la VM (el compilador lo emite),
//! - la **regla de tipado**: valida aridad y tipos de los argumentos ya comprobados y da el tipo
//!   de retorno.
//!
//! Las *implementaciones de ejecución* siguen donde corresponde (el `match` por opcode en la VM y
//! `eval_builtin` en el intérprete): son código específico de cada motor, no metadatos. Pero la
//! membresía, las firmas y el mapeo a opcode —lo duplicado y propenso a desincronizarse— están
//! centralizados aquí. Añadir un builtin "normal" es ahora: una fila en esta tabla + su opcode en
//! la VM + su caso en `eval_builtin`.
//!
//! Nota: cuatro builtins son **ad-hoc polimórficos** y no tendrían una firma raylang ordinaria
//! (`print`/`eprint` aceptan cualquier imprimible; `len` un arreglo *o* string; `to_string`
//! int/float/bool/string). Por eso la regla es una función y no una firma fija: cada uno expresa su
//! propio criterio. Es la razón por la que se eligió esta tabla en Rust frente a un `@builtin fn`.

use crate::ast::Type;
use crate::bytecode::OpCode;

/// Error de tipado de un builtin: `(índice_del_arg, mensaje)`. El índice `None` señala un error
/// general de la llamada (p. ej. aridad); `Some(i)` el argumento culpable (para ubicar el cursor).
pub type BuiltinError = (Option<usize>, String);

/// La regla de tipado de un builtin: de los tipos de los argumentos ya comprobados al tipo del
/// resultado (o un error).
pub type CheckFn = fn(&[Type]) -> Result<Type, BuiltinError>;

/// La especificación de un builtin: cómo se llama, qué opcode lo ejecuta y cómo se tipa.
pub struct Builtin {
    pub name: &'static str,
    pub opcode: OpCode,
    pub check: CheckFn,
}

/// Busca un builtin por nombre.
pub fn lookup(name: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.name == name)
}

/// ¿`name` nombra un builtin?
pub fn is_builtin(name: &str) -> bool {
    lookup(name).is_some()
}

/// Los nombres de todos los builtins (incluidos los internos `__*`). Lo usa el LSP para autocompletar
/// (filtrando los `__*`, que son primitivos no destinados al usuario).
pub fn names() -> impl Iterator<Item = &'static str> {
    BUILTINS.iter().map(|b| b.name)
}

/// Añade `contents` al final del archivo `path` (lo crea si no existe). Helper compartido por ambos
/// motores para el primitivo `__append_file` (M11.4b); la *impl* de ejecución no es metadato, pero
/// es idéntica en los dos motores, así que vive aquí para no duplicarse.
pub fn append_to_file(path: &str, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(contents.as_bytes())
}

// --- Helpers de las reglas ---

/// Error de aridad "espera N argumento(s), se le pasaron M".
fn arity(a: &[Type], n: usize, nombre: &str, detalle: &str) -> Result<(), BuiltinError> {
    if a.len() != n {
        let plural = if n == 1 { "argumento" } else { "argumentos" };
        return Err((None, format!("{} espera {} {}{}, se le pasaron {}", nombre, n, plural, detalle, a.len())));
    }
    Ok(())
}

/// Error de aridad para builtins sin argumentos.
fn nullary(a: &[Type], nombre: &str) -> Result<(), BuiltinError> {
    if !a.is_empty() {
        return Err((None, format!("{} no espera argumentos, se le pasaron {}", nombre, a.len())));
    }
    Ok(())
}

/// ¿Es un tipo que `print`/`eprint` saben imprimir? (Coincide con `is_printable` del checker.)
fn printable(t: &Type) -> bool {
    matches!(
        t,
        Type::Int | Type::Float | Type::Bool | Type::String | Type::Char | Type::Array(_)
            | Type::Struct(_, _) | Type::Fn(_, _) | Type::Enum(_, _) | Type::Var(_)
    )
}

/// La tabla. El orden no importa (la búsqueda es por nombre).
static BUILTINS: &[Builtin] = &[
    // print(x) -> unit: imprime un imprimible a stdout.
    Builtin { name: "print", opcode: OpCode::Print, check: |a| {
        arity(a, 1, "print", "")?;
        if !printable(&a[0]) { return Err((Some(0), format!("print no puede imprimir un {}", a[0]))); }
        Ok(Type::Unit)
    } },
    // len(a) -> int: longitud de un arreglo o de un string (M11.1a: nº de caracteres).
    Builtin { name: "len", opcode: OpCode::Len, check: |a| {
        arity(a, 1, "len", "")?;
        if !matches!(a[0], Type::Array(_) | Type::String) {
            return Err((Some(0), format!("len espera un arreglo o un string, no {}", a[0])));
        }
        Ok(Type::Int)
    } },
    // push(a, x) -> unit: agrega x al final del arreglo a (lo muta).
    Builtin { name: "push", opcode: OpCode::Push, check: |a| {
        arity(a, 2, "push", " (arreglo, valor)")?;
        let elem = match &a[0] {
            Type::Array(e) => (**e).clone(),
            other => return Err((Some(0), format!("push espera un arreglo como primer argumento, no {}", other))),
        };
        if a[1] != elem {
            return Err((Some(1), format!("push: el arreglo es de {} pero se empuja {}", elem, a[1])));
        }
        Ok(Type::Unit)
    } },
    // to_string(x) -> string (M11.1a): representación textual de un primitivo imprimible.
    Builtin { name: "to_string", opcode: OpCode::ToString, check: |a| {
        arity(a, 1, "to_string", "")?;
        if !matches!(a[0], Type::Int | Type::Float | Type::Bool | Type::String | Type::Char) {
            return Err((Some(0), format!("to_string solo convierte int/float/bool/string/char, no {}", a[0])));
        }
        Ok(Type::String)
    } },
    // trim(s) -> string (M11.1b): quita el espacio en blanco de los extremos.
    Builtin { name: "trim", opcode: OpCode::Trim, check: |a| {
        arity(a, 1, "trim", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("trim espera un string, no {}", a[0]))); }
        Ok(Type::String)
    } },
    // split(s, sep) -> [string] (M11.1b): parte s por el separador sep.
    Builtin { name: "split", opcode: OpCode::Split, check: |a| {
        arity(a, 2, "split", " (string, separador)")?;
        if a[0] != Type::String { return Err((Some(0), format!("split espera un string como primer argumento, no {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("split espera un string como separador, no {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // contains(s, sub) -> bool (M11.4a): ¿s contiene la subcadena sub?
    Builtin { name: "contains", opcode: OpCode::Contains, check: |a| {
        arity(a, 2, "contains", " (string, subcadena)")?;
        if a[0] != Type::String { return Err((Some(0), format!("contains espera un string como primer argumento, no {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("contains espera un string como subcadena, no {}", a[1]))); }
        Ok(Type::Bool)
    } },
    // replace(s, de, a) -> string (M11.4a): reemplaza todas las ocurrencias de `de` por `a`.
    Builtin { name: "replace", opcode: OpCode::Replace, check: |a| {
        arity(a, 3, "replace", " (string, de, a)")?;
        if a[0] != Type::String { return Err((Some(0), format!("replace espera un string como primer argumento, no {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("replace espera un string en 'de', no {}", a[1]))); }
        if a[2] != Type::String { return Err((Some(2), format!("replace espera un string en 'a', no {}", a[2]))); }
        Ok(Type::String)
    } },
    // chars(s) -> [char] (M11.4c-2): los caracteres del string, en orden.
    Builtin { name: "chars", opcode: OpCode::Chars, check: |a| {
        arity(a, 1, "chars", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("chars espera un string, no {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::Char)))
    } },
    // eprint(x) -> unit (M11.2a): como print, pero a stderr.
    Builtin { name: "eprint", opcode: OpCode::EPrint, check: |a| {
        arity(a, 1, "eprint", "")?;
        if !printable(&a[0]) { return Err((Some(0), format!("eprint no puede imprimir un {}", a[0]))); }
        Ok(Type::Unit)
    } },
    // __parse_int(s) -> [int] (M11.2a): [] si no parsea, [n] si sí. El prelude → Option<int>.
    Builtin { name: "__parse_int", opcode: OpCode::ParseInt, check: |a| {
        arity(a, 1, "__parse_int", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__parse_int espera un string, no {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::Int)))
    } },
    // __read_line() -> [string] (M11.2a): [] en EOF, [linea] si no. El prelude → Option<string>.
    Builtin { name: "__read_line", opcode: OpCode::ReadLine, check: |a| {
        nullary(a, "__read_line")?;
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __env(s) -> [string] (M11.2b): [] si no existe, [valor] si sí. El prelude → Option<string>.
    Builtin { name: "__env", opcode: OpCode::Env, check: |a| {
        arity(a, 1, "__env", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__env espera un string, no {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // args() -> [string] (M11.2b): argumentos de la línea de comandos del programa.
    Builtin { name: "args", opcode: OpCode::Args, check: |a| {
        nullary(a, "args")?;
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __read_file(path) -> [string] (M11.2c): ["ok", contenido] o ["err", msg]. Prelude → Result.
    Builtin { name: "__read_file", opcode: OpCode::ReadFile, check: |a| {
        arity(a, 1, "__read_file", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("__read_file espera un string (la ruta), no {}", a[0]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // __write_file(path, contenido) -> [string] (M11.2c): ["ok"] o ["err", msg]. Prelude → Result.
    Builtin { name: "__write_file", opcode: OpCode::WriteFile, check: |a| {
        arity(a, 2, "__write_file", " (ruta, contenido)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__write_file espera un string (la ruta), no {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__write_file espera un string (el contenido), no {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
    // exists(ruta) -> bool (M11.4b): ¿existe la ruta? Total (no falla).
    Builtin { name: "exists", opcode: OpCode::Exists, check: |a| {
        arity(a, 1, "exists", "")?;
        if a[0] != Type::String { return Err((Some(0), format!("exists espera un string (la ruta), no {}", a[0]))); }
        Ok(Type::Bool)
    } },
    // __append_file(path, contenido) -> [string] (M11.4b): ["ok"] o ["err", msg]. Prelude → Result.
    Builtin { name: "__append_file", opcode: OpCode::AppendFile, check: |a| {
        arity(a, 2, "__append_file", " (ruta, contenido)")?;
        if a[0] != Type::String { return Err((Some(0), format!("__append_file espera un string (la ruta), no {}", a[0]))); }
        if a[1] != Type::String { return Err((Some(1), format!("__append_file espera un string (el contenido), no {}", a[1]))); }
        Ok(Type::Array(Box::new(Type::String)))
    } },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membresia() {
        assert!(is_builtin("print"));
        assert!(is_builtin("args"));
        assert!(!is_builtin("noexiste"));
        assert!(!is_builtin("map")); // map/filter/fold son del prelude, no builtins
    }

    #[test]
    fn regla_ok_y_errores() {
        let split = lookup("split").unwrap();
        // Firma correcta → tipo de retorno.
        assert_eq!((split.check)(&[Type::String, Type::String]), Ok(Type::Array(Box::new(Type::String))));
        // Aridad mal → error general (índice None: lo ubica el sitio de llamada).
        assert!(matches!((split.check)(&[Type::String]), Err((None, _))));
        // Tipo de un arg mal → error con el índice del argumento culpable.
        assert!(matches!((split.check)(&[Type::Int, Type::String]), Err((Some(0), _))));
    }

    #[test]
    fn push_es_homogeneo() {
        let push = lookup("push").unwrap();
        let xs_int = Type::Array(Box::new(Type::Int));
        assert_eq!((push.check)(&[xs_int.clone(), Type::Int]), Ok(Type::Unit));
        assert!(matches!((push.check)(&[xs_int, Type::String]), Err((Some(1), _))));
    }
}
