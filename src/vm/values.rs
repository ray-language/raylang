//! Conversión y formato de valores de la VM (movimiento puro; usar `git log --follow`).
//!
//! `const_to_heap`/`to_value` cruzan el borde constante-de-chunk ↔ heap y VM ↔ intérprete;
//! `values_equal`/`format_value` son igualdad estructural y `Display`; `heap_to_key`/
//! `key_to_heap` cruzan a/desde `MapKey` (M13.1).

use super::*;

/// M28.3: construye un `HeapValue::UInt` enmascarando al ancho (aplica el wrapping), como
/// `make_uint` del intérprete.
pub(super) fn uint_heap(val: u64, width: u8) -> HeapValue {
    HeapValue::UInt(val & crate::runtime::uint_mask(width), width)
}

/// Convierte una constante del chunk (un `Value` del intérprete, siempre primitivo)
/// al valor de la VM.
pub(super) fn const_to_heap(v: &Value) -> HeapValue {
    match v {
        Value::Int(n) => HeapValue::Int(*n),
        Value::Float(x) => HeapValue::Float(*x),
        Value::Bool(b) => HeapValue::Bool(*b),
        Value::Str(s) => HeapValue::Str(s.clone()),
        Value::Char(c) => HeapValue::Char(*c),
        Value::UInt(n, w) => HeapValue::UInt(*n, *w), // M28.3
        Value::Bytes(b) => HeapValue::Bytes((**b).clone()),
        Value::Unit => HeapValue::Unit,
        _ => unreachable!("chunk constants are primitive"),
    }
}

/// Igualdad estructural entre valores de la VM (mira el heap). Las funciones y
/// closures se comparan por identidad (el checker prohíbe `==` sobre ellas).
pub(super) fn values_equal(heap: &Heap, a: &HeapValue, b: &HeapValue) -> bool {
    use HeapValue as H;
    match (a, b) {
        (H::Int(x), H::Int(y)) => x == y,
        (H::Float(x), H::Float(y)) => x == y,
        (H::Bool(x), H::Bool(y)) => x == y,
        (H::Str(x), H::Str(y)) => x == y,
        (H::Char(x), H::Char(y)) => x == y,
        (H::UInt(x, _), H::UInt(y, _)) => x == y, // M28.3 (mismo ancho garantizado por el checker)
        (H::Bytes(x), H::Bytes(y)) => x == y,
        (H::Ptr(x), H::Ptr(y)) => x == y, // M41.4b: identidad de puntero
        (H::Unit, H::Unit) => true,
        (H::Function(x), H::Function(y)) => x == y,
        (H::Obj(x), H::Obj(y)) => match (heap.get(*x), heap.get(*y)) {
            (Obj::Array(va), Obj::Array(vb)) => {
                va.len() == vb.len() && va.iter().zip(vb).all(|(p, q)| values_equal(heap, p, q))
            }
            // M98.5: IntArray puro y mixto (un [int] puede estar en cualquiera de las dos formas).
            (Obj::IntArray(va), Obj::IntArray(vb)) => va == vb,
            (Obj::IntArray(va), Obj::Array(vb)) | (Obj::Array(vb), Obj::IntArray(va)) => {
                va.len() == vb.len()
                    && va.iter().zip(vb).all(|(p, q)| matches!(q, H::Int(n) if n == p))
            }
            (Obj::Struct(sa), Obj::Struct(sb)) => {
                // TA1: mismo índice de def ≡ mismo tipo con los mismos campos (los nombres viven en
                // la tabla); basta comparar los valores en orden.
                sa.struct_idx == sb.struct_idx
                    && sa.fields.len() == sb.fields.len()
                    && sa.fields.iter().zip(&sb.fields).all(|(v1, v2)| values_equal(heap, v1, v2))
            }
            // Closures: identidad (mismo handle).
            (Obj::Closure(_), Obj::Closure(_)) => x == y,
            _ => false,
        },
        _ => false,
    }
}

/// Resuelve `(enum_id, tag)` de un enum a `(nombre_enum, nombre_variante)` usando la
/// tabla de enums del programa.
pub(super) fn enum_names<'a>(enums: &'a [CompiledEnum], enum_id: usize, tag: usize) -> (&'a str, &'a str) {
    let e = &enums[enum_id];
    (e.name.as_str(), e.variants[tag].name.as_str())
}

/// Formatea un valor de la VM como texto (siguiendo handles en el heap). Debe
/// coincidir con el `Display` del `Value` del intérprete, para que `print` sea igual.
pub(super) fn format_value(heap: &Heap, structs: &[crate::bytecode::CompiledStruct], enums: &[CompiledEnum], v: &HeapValue) -> String {
    match v {
        HeapValue::Int(n) => n.to_string(),
        HeapValue::Float(x) => x.to_string(),
        HeapValue::Bool(b) => b.to_string(),
        HeapValue::Str(s) => s.clone(),
        HeapValue::Char(c) => c.to_string(),
        HeapValue::UInt(n, _) => n.to_string(),
        HeapValue::Bytes(b) => crate::builtins::bytes_to_hex(b),
        HeapValue::Ptr(_) => "<ptr>".to_string(), // M41.4b: dirección no determinista → repr opaca
        HeapValue::Unit => "()".to_string(),
        HeapValue::Function(_) => "<fn>".to_string(),
        HeapValue::Obj(h) => match heap.get(*h) {
            Obj::Array(elems) => {
                let parts: Vec<String> = elems.iter().map(|e| format_value(heap, structs, enums, e)).collect();
                format!("[{}]", parts.join(", "))
            }
            // M98.5: misma repr que el genérico (la forma de almacenamiento es invisible).
            Obj::IntArray(v) => {
                let parts: Vec<String> = v.iter().map(|i| i.to_string()).collect();
                format!("[{}]", parts.join(", "))
            }
            Obj::Struct(s) => {
                let def = &structs[s.struct_idx];
                let parts: Vec<String> = def.fields.iter().zip(&s.fields)
                    .map(|(n, v)| format!("{}: {}", n, format_value(heap, structs, enums, v))).collect();
                format!("{} {{ {} }}", def.name, parts.join(", "))
            }
            Obj::Enum(e) => {
                let (ename, vname) = enum_names(enums, e.enum_id as usize, e.tag as usize);
                if e.payload.is_empty() {
                    format!("{}.{}", ename, vname)
                } else {
                    let parts: Vec<String> = e.payload.iter().map(|v| format_value(heap, structs, enums, v)).collect();
                    format!("{}.{}({})", ename, vname, parts.join(", "))
                }
            }
            Obj::Closure(_) => "<fn>".to_string(),
            Obj::Cell(_) => "<cell>".to_string(), // no debería imprimirse directamente
            // M13.1: el print de un Map está diferido; se ordena por clave (determinista).
            Obj::Map(m) => {
                let mut parts: Vec<String> = m.iter()
                    .map(|(k, v)| format!("{}: {}", k.to_value(), format_value(heap, structs, enums, v)))
                    .collect();
                parts.sort();
                format!("Map{{{}}}", parts.join(", "))
            }
        },
        // M38.1b: canal/tarea (host) no se inspeccionan textualmente.
        HeapValue::Channel(_) => "<channel>".to_string(),
        HeapValue::Task(_) => "<task>".to_string(),
    }
}

/// Convierte un valor de la VM al `Value` del intérprete (para el resultado final y
/// el oráculo). Los compuestos se reconstruyen siguiendo el heap.
pub(super) fn to_value(heap: &Heap, structs: &[crate::bytecode::CompiledStruct], enums: &[CompiledEnum], v: &HeapValue) -> Value {
    match v {
        HeapValue::Int(n) => Value::Int(*n),
        HeapValue::Float(x) => Value::Float(*x),
        HeapValue::Bool(b) => Value::Bool(*b),
        HeapValue::Str(s) => Value::Str(s.clone()),
        HeapValue::Char(c) => Value::Char(*c),
        HeapValue::UInt(n, w) => Value::UInt(*n, *w),
        HeapValue::Bytes(b) => Value::Bytes(Rc::new(b.clone())),
        HeapValue::Ptr(p) => Value::Ptr(*p), // M41.4b
        HeapValue::Unit => Value::Unit,
        HeapValue::Function(i) => Value::Function(*i),
        HeapValue::Obj(h) => match heap.get(*h) {
            Obj::Array(elems) => {
                let v: Vec<Value> = elems.iter().map(|e| to_value(heap, structs, enums, e)).collect();
                Value::Array(Rc::new(RefCell::new(v)))
            }
            // M98.5: al borde se convierte igual que el genérico (invisible para el intérprete).
            Obj::IntArray(xs) => {
                let v: Vec<Value> = xs.iter().map(|&i| Value::Int(i)).collect();
                Value::Array(Rc::new(RefCell::new(v)))
            }
            Obj::Struct(s) => {
                let def = &structs[s.struct_idx];
                let fields: Vec<(String, Value)> = def.fields.iter().zip(&s.fields)
                    .map(|(n, v)| (n.clone(), to_value(heap, structs, enums, v))).collect();
                Value::Struct(Rc::new(RefCell::new(StructInstance { name: def.name.clone(), fields })))
            }
            Obj::Enum(e) => {
                let (ename, vname) = enum_names(enums, e.enum_id as usize, e.tag as usize);
                let payload: Vec<Value> = e.payload.iter().map(|v| to_value(heap, structs, enums, v)).collect();
                Value::Enum(Rc::new(EnumInstance {
                    enum_name: ename.to_string(),
                    variant: vname.to_string(),
                    payload,
                }))
            }
            // Una closure como resultado: la representamos como función (su identidad
            // no se observa; se imprime <fn>).
            Obj::Closure(c) => Value::Function(c.index),
            Obj::Cell(inner) => to_value(heap, structs, enums, inner),
            // M13.1: reconstruye el Map del intérprete (igual igualdad estructural → oráculo).
            Obj::Map(m) => {
                let mut hm = crate::runtime::MapStore::with_capacity_and_hasher(m.len(), Default::default());
                for (k, val) in m.iter() {
                    hm.insert(k.clone(), to_value(heap, structs, enums, val));
                }
                Value::Map(Rc::new(RefCell::new(hm)))
            }
        },
        // M38.1b: un canal/tarea (host) nunca es el resultado del programa ni cruza al intérprete
        // (main devuelve int/unit; no hay oráculo concurrente).
        HeapValue::Channel(_) => unreachable!("a channel is never the program result"),
        HeapValue::Task(_) => unreachable!("a task is never the program result"),
    }
}

/// Convierte un valor de la VM en una clave de Map (M13.1). El checker garantiza el tipo.
/// V4 (bench políglota): **consume** el valor — todos los sitios lo llaman con el valor recién
/// sacado de la pila (owned), así el String/Bytes de la clave se MUEVE en vez de clonarse
/// (antes: 1 alloc+copia por cada insert/get/contains/remove/add_to con clave string).
pub(super) fn heap_to_key(v: HeapValue) -> MapKey {
    match v {
        HeapValue::Int(n) => MapKey::Int(n),
        HeapValue::Str(s) => MapKey::Str(s),
        HeapValue::Char(c) => MapKey::Char(c),
        HeapValue::Bool(b) => MapKey::Bool(b),
        HeapValue::Bytes(b) => MapKey::Bytes(b),
        _ => unreachable!("the checker guarantees a hashable key (int/string/char/bool/bytes)"),
    }
}

/// Reconstruye el valor de la VM a partir de una clave de Map (para `keys`, M13.1b).
pub(super) fn key_to_heap(k: &MapKey) -> HeapValue {
    match k {
        MapKey::Int(n) => HeapValue::Int(*n),
        MapKey::Str(s) => HeapValue::Str(s.clone()),
        MapKey::Char(c) => HeapValue::Char(*c),
        MapKey::Bool(b) => HeapValue::Bool(*b),
        MapKey::Bytes(b) => HeapValue::Bytes(b.clone()),
    }
}
