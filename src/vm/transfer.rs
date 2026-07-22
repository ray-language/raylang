//! M38.1a — transferencia de subgrafo entre heaps (movimiento puro; usar `git log --follow`).
//!
//! El ladrillo del aislamiento por actores: copiar el subgrafo de un valor de un heap a otro,
//! remapeando los handles (`send`/`recv`/`spawn`, M38.1b).

use super::*;

// ===================== M38.1a — transferencia de subgrafo entre heaps =====================
//
// El ladrillo del aislamiento por actores (§46): copiar el subgrafo de un valor de un heap a otro,
// remapeando los handles. Lo usará `send`/`recv`/`spawn` cuando cada fibra tenga su heap (M38.1b): un
// valor que cruza de un actor a otro se **re-aloja** en el heap destino. Aquí se construye y prueba en
// AISLAMIENTO (dos heaps sueltos), sin tocar el VM en marcha.
//
// Correcto ante **sharing y ciclos**: un `remap: old_handle → new_handle` memoiza los objetos ya
// copiados, así (a) el sharing interno del subgrafo se preserva (un objeto alcanzado por dos caminos se
// copia una vez) y (b) los ciclos (las closures capturan celdas → grafos cíclicos) no ciclan al copiar.
// Para cerrar el ciclo se **reserva primero** un placeholder en el destino y se registra el mapeo ANTES
// de copiar los hijos, de modo que una referencia de vuelta encuentre el handle nuevo.
//
// `Channel`/`Task` NO se transfieren: son la sincronización **compartida** entre actores (viven fuera del
// heap de cualquier fibra; §46.2). Aquí es un caso inalcanzable (los valores que cruzan son datos).

/// Transfiere `v` del heap `src` al heap `dst`, devolviendo el valor equivalente con handles del destino.
/// Los primitivos se copian tal cual; los objetos se re-alojan (ver `transfer_obj`).
pub(super) fn transfer_value(
    src: &Heap,
    dst: &mut Heap,
    v: &HeapValue,
    remap: &mut HashMap<Handle, Handle>,
) -> HeapValue {
    match v {
        HeapValue::Obj(h) => HeapValue::Obj(transfer_obj(src, dst, *h, remap)),
        // Escalares inline (Int/Float/Bool/Str/Char/UInt/Bytes/Ptr/Unit/Function): copia directa.
        other => other.clone(),
    }
}

/// Re-aloja el objeto `h` de `src` en `dst`, recursivamente. Reserva un placeholder + registra el mapeo
/// antes de copiar los hijos (para ciclos), y memoiza (para sharing).
/// M98.5: elige la forma de almacenamiento de un arreglo nuevo — todos los elementos `Int` →
/// `IntArray` compacto (8 B/elem); si no, genérico. Fuera del bucle de despacho a propósito
/// (el tamaño del cuerpo del match afecta el layout de los caminos calientes, cf. P0.6).
pub(super) fn specialize_array(elems: Vec<HeapValue>) -> Obj {
    if !elems.is_empty() && elems.iter().all(|e| matches!(e, HeapValue::Int(_))) {
        Obj::IntArray(elems.iter().map(|e| match e {
            HeapValue::Int(i) => *i,
            _ => unreachable!("just checked all Int"),
        }).collect())
    } else {
        Obj::Array(elems)
    }
}

pub(super) fn transfer_obj(src: &Heap, dst: &mut Heap, h: Handle, remap: &mut HashMap<Handle, Handle>) -> Handle {
    if let Some(&nh) = remap.get(&h) {
        return nh; // ya copiado (sharing o ciclo) → reusa el handle destino
    }
    // Reserva un placeholder y registra el mapeo ANTES de recursar (cierra los ciclos).
    let nh = dst.allocate(Obj::Array(Vec::new()));
    remap.insert(h, nh);
    // Se **clona** la estructura del objeto origen para soltar el préstamo de `src` antes de transferir
    // los hijos (que mutan `dst`). El clon copia los `HeapValue` hijos (baratos salvo Str/Bytes); sus
    // handles se remapean al transferirlos.
    let new_obj: Obj = match src.get(h) {
        Obj::Array(elems) => {
            let elems = elems.clone();
            Obj::Array(elems.iter().map(|e| transfer_value(src, dst, e, remap)).collect())
        }
        // M98.5: sin handles que remapear → copia directa (y cruza los hilos ya compacto).
        Obj::IntArray(v) => Obj::IntArray(v.clone()),
        Obj::Struct(s) => {
            let struct_idx = s.struct_idx;
            let fields = s.fields.clone();
            Obj::Struct(VmStruct {
                struct_idx,
                fields: fields.iter().map(|e| transfer_value(src, dst, e, remap)).collect(),
            })
        }
        Obj::Enum(e) => {
            let (enum_id, tag) = (e.enum_id, e.tag);
            let payload = e.payload.clone();
            Obj::Enum(VmEnum {
                enum_id,
                tag,
                payload: payload.iter().map(|e| transfer_value(src, dst, e, remap)).collect(),
            })
        }
        Obj::Closure(c) => {
            let index = c.index;
            let upvalues = c.upvalues.clone(); // handles a celdas
            Obj::Closure(VmClosure {
                index,
                upvalues: upvalues.iter().map(|&up| transfer_obj(src, dst, up, remap)).collect(),
            })
        }
        Obj::Cell(inner) => {
            let inner = inner.clone();
            Obj::Cell(transfer_value(src, dst, &inner, remap))
        }
        Obj::Map(m) => {
            let pairs: Vec<(MapKey, HeapValue)> = m.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            let mut nm = crate::gc::MapStore::with_capacity_and_hasher(pairs.len(), Default::default());
            for (k, val) in pairs {
                let nv = transfer_value(src, dst, &val, remap);
                nm.insert(k, nv); // las claves son primitivos (sin handles)
            }
            Obj::Map(Box::new(nm))
        }
    };
    *dst.get_mut(nh) = new_obj;
    nh
}
