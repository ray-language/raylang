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
    ("std/fs", include_str!("../std/fs.ray")), // M50.1
    ("std/math", include_str!("../std/math.ray")),
    ("std/random", include_str!("../std/random.ray")), // M49.2a
    ("std/time", include_str!("../std/time.ray")), // M49.2b
    ("std/crypto", include_str!("../std/crypto.ray")), // M49.3
    ("std/text", include_str!("../std/text.ray")),
    ("std/sort", include_str!("../std/sort.ray")),
    // Librerías **promovidas** desde `examples/` (M40.7): se embeben apuntando al archivo original —fuente
    // ÚNICA, sin duplicar—; el ejemplo sigue siendo el artefacto pedagógico y a la vez la fuente de `std/`.
    // Solo se promueven así las **hojas** (sin `import`); las que tienen dependencias se namespacan (M40.7b+).
    ("std/hex", include_str!("../examples/web/hex.ray")),
    ("std/base64", include_str!("../examples/web/base64.ray")),
    ("std/url", include_str!("../examples/web/url.ray")),
    ("std/json", include_str!("../examples/web/json.ray")),
    // NOTA (M43.5b): la **criptografía** (sha1/sha256/sha512/hmac/chacha20/poly1305/chacha20poly1305/
    // ed25519) YA NO se embebe. La cripto de producción son los **builtins de `ring`** (tiempo constante,
    // auditados; M43); las implementaciones en raylang puro (`examples/web/sha256.ray`, etc.) se conservan
    // SOLO como demostración del lenguaje (correctas, pero sobre la VM no garantizan resistencia a canales
    // laterales). El paquete `net` las consume vía `net/crypto` (adaptadores sobre los builtins).
    // Compresión (M40.7c). `deflate` → `std/inflate` (namespacado en el ejemplo).
    ("std/inflate", include_str!("../examples/web/inflate.ray")),
    ("std/deflate", include_str!("../examples/web/deflate.ray")),
    ("std/huffman", include_str!("../examples/web/huffman.ray")),
    // Procesamiento de texto/datos (M40.7d): librerías puras de `examples/stdlib/` (todas hojas).
    ("std/regex", include_str!("../examples/stdlib/regex.ray")),
    ("std/csv", include_str!("../examples/stdlib/csv.ray")),
    ("std/toml", include_str!("../examples/stdlib/toml.ray")),
    ("std/template", include_str!("../examples/stdlib/template.ray")),
    // Protobuf (M40.7e; la cripto que lo acompañaba se des-embebió, ver la nota de arriba).
    ("std/protobuf", include_str!("../examples/web/protobuf.ray")),
    // UUID (M40.7f): `uuid_v4` usa `random_int` (no determinista), `is_uuid_v4` valida. → `std/hex`.
    ("std/uuid", include_str!("../examples/web/uuid.ray")),
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
