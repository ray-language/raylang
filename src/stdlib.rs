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
    // Utilidades escritas para la stdlib (viven en `std/`).
    ("std/math", include_str!("../std/math.ray")),
    ("std/text", include_str!("../std/text.ray")),
    ("std/sort", include_str!("../std/sort.ray")),
    // Librerías **promovidas** desde `examples/` (M40.7): se embeben apuntando al archivo original —fuente
    // ÚNICA, sin duplicar—; el ejemplo sigue siendo el artefacto pedagógico y a la vez la fuente de `std/`.
    // Solo se promueven así las **hojas** (sin `import`); las que tienen dependencias se namespacan (M40.7b+).
    ("std/hex", include_str!("../examples/web/hex.ray")),
    ("std/base64", include_str!("../examples/web/base64.ray")),
    ("std/url", include_str!("../examples/web/url.ray")),
    ("std/json", include_str!("../examples/web/json.ray")),
    // Hashing (M40.7b). `sha512`/`hmac` no son hojas: sus `from … import` se namespacaron a `std/…` (en el
    // propio ejemplo), que la resolución embebida satisface corran donde corran.
    ("std/sha1", include_str!("../examples/web/sha1.ray")),
    ("std/sha256", include_str!("../examples/web/sha256.ray")),
    ("std/sha512", include_str!("../examples/web/sha512.ray")),
    ("std/hmac", include_str!("../examples/web/hmac.ray")),
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
        assert!(embedded("std/sort").is_some());
        assert!(embedded("std/hex").is_some());
        assert!(embedded("std/json").is_some());
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
