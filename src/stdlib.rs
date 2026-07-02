//! La biblioteca estándar `std/`, **embebida en el binario** (M40.5).
//!
//! A diferencia del **prelude** (que se inyecta automáticamente en cada programa), la `std/` es
//! **opcional**: solo se carga lo que se importa con `import std/…;`. Hasta M40.4 los módulos vivían
//! en disco (descubiertos por el CLI subiendo desde el ejecutable o vía `RAYLANG_STD`); esto ataba el
//! binario a un layout de archivos. M40.5 los **empaqueta** con `include_str!` —igual que el prelude—,
//! así el ejecutable es **auto-contenido**: `ray run prog.ray` con `import std/math;` funciona sin que
//! `std/` exista en disco.
//!
//! Los `std/*.ray` del repo siguen siendo la **única fuente de verdad**: `include_str!` los compila
//! dentro, y `ray doc std/math.ray` los lee directamente del disco (en el repo). Añadir un módulo =
//! una fila en [`MODULOS`].
//!
//! El [`loader`](crate::loader) consulta [`embedded`] **antes** de tocar el disco: si el nombre de
//! módulo (`std/math`) está aquí, usa la fuente embebida y no busca en el filesystem. El prefijo
//! `std/` queda así **reservado**.

/// Los módulos embebidos: `(nombre de módulo, fuente)`. El nombre es la **ruta de import** sin `.ray`
/// (`import std/math;` → `"std/math"`). Las rutas de `include_str!` son relativas a este archivo
/// (`src/stdlib.rs` → `../std/…`).
const MODULOS: &[(&str, &str)] = &[
    ("std/math", include_str!("../std/math.ray")),
    ("std/text", include_str!("../std/text.ray")),
];

/// La fuente embebida del módulo `nombre` (`"std/math"`), o `None` si no es un módulo de la stdlib.
pub fn embedded(nombre: &str) -> Option<&'static str> {
    MODULOS.iter().find(|(n, _)| *n == nombre).map(|(_, src)| *src)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modulos_conocidos_resuelven() {
        assert!(embedded("std/math").is_some());
        assert!(embedded("std/text").is_some());
    }

    #[test]
    fn desconocido_es_none() {
        assert!(embedded("std/inexistente").is_none());
        assert!(embedded("math").is_none()); // sin el prefijo std/ no es de la stdlib
    }

    #[test]
    fn la_fuente_embebida_no_esta_vacia() {
        assert!(embedded("std/math").unwrap().contains("pub fn gcd"));
        assert!(embedded("std/text").unwrap().contains("pub fn capitalize"));
    }
}
