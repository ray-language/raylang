//! M458 (pepa #418): los idiomas preferidos del sistema, como etiquetas BCP 47 (`es-CL`, `en-US`)
//! por orden de preferencia. `std/locale` → `preferred_languages()`.
//!
//! Una sola fuente para la VM y para el binario nativo: el transpilador pega este archivo como
//! módulo del programa (como `float_fmt.rs`), así que solo usa `std` y FFI declarada aquí.
//!
//! - `RAY_LANGUAGES="es-CL,en"` manda en todas partes: pruebas y entornos sin sesión de usuario.
//! - macOS / iOS: `CFLocaleCopyPreferredLanguages` (lo que el usuario ordenó en Ajustes).
//! - Android: la propiedad del sistema `persist.sys.locale` (o `ro.product.locale`).
//! - Windows: `GetUserPreferredUILanguages`.
//! - Resto de Unix: `LANGUAGE` (lista), `LC_ALL`, `LC_MESSAGES`, `LANG`.
//!
//! Nunca falla: sin información devuelve una lista vacía.

/// Los idiomas preferidos, sin repetidos y en orden.
pub fn preferred_languages() -> Vec<String> {
    let raw = match std::env::var("RAY_LANGUAGES") {
        Ok(list) if !list.trim().is_empty() => list.split(',').map(str::to_string).collect(),
        _ => system_languages(),
    };
    let mut out: Vec<String> = Vec::new();
    for item in raw {
        if let Some(tag) = normalize(&item) {
            if !out.contains(&tag) {
                out.push(tag);
            }
        }
    }
    out
}

/// De la forma POSIX (`es_CL.UTF-8@euro`) o BCP 47 a BCP 47: sin codificación ni modificador,
/// con guiones, el idioma en minúsculas y la región de dos letras en mayúsculas. `C` y `POSIX`
/// no son idiomas.
pub fn normalize(raw: &str) -> Option<String> {
    let base = raw.trim().split(['.', '@']).next().unwrap_or("");
    if base.is_empty() || base.eq_ignore_ascii_case("C") || base.eq_ignore_ascii_case("POSIX") {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    for (i, part) in base.split(['_', '-']).enumerate() {
        if part.is_empty() || !part.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        parts.push(if i == 0 {
            part.to_ascii_lowercase()
        } else if part.len() == 2 {
            part.to_ascii_uppercase() // región: es-cl → es-CL
        } else if part.len() == 4 && part.chars().all(|c| c.is_ascii_alphabetic()) {
            // escritura: zh-hans → zh-Hans
            let mut s = part.to_ascii_lowercase();
            s[..1].make_ascii_uppercase();
            s
        } else {
            part.to_string()
        });
    }
    Some(parts.join("-"))
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn system_languages() -> Vec<String> {
    use std::ffi::c_void;
    type CFRef = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFLocaleCopyPreferredLanguages() -> CFRef;
        fn CFArrayGetCount(array: CFRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFRef, index: isize) -> CFRef;
        fn CFStringGetLength(s: CFRef) -> isize;
        fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
        fn CFStringGetCString(s: CFRef, buffer: *mut u8, size: isize, encoding: u32) -> u8;
        fn CFRelease(obj: CFRef);
    }
    let mut out = Vec::new();
    // SAFETY: la función «Copy» devuelve un CFArray propio (o NULL), que se libera al final; los
    // elementos son CFString que el arreglo retiene mientras vive, y cada uno se copia a un búfer
    // del tamaño que CoreFoundation pide para UTF-8 más el terminador.
    unsafe {
        let array = CFLocaleCopyPreferredLanguages();
        if array.is_null() {
            return out;
        }
        for i in 0..CFArrayGetCount(array) {
            let s = CFArrayGetValueAtIndex(array, i);
            if s.is_null() {
                continue;
            }
            let size = CFStringGetMaximumSizeForEncoding(CFStringGetLength(s), UTF8) + 1;
            let mut buf = vec![0u8; size.max(1) as usize];
            if CFStringGetCString(s, buf.as_mut_ptr(), size, UTF8) != 0 {
                let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
                out.push(String::from_utf8_lossy(&buf[..end]).into_owned());
            }
        }
        CFRelease(array);
    }
    out
}

#[cfg(target_os = "android")]
fn system_languages() -> Vec<String> {
    use std::ffi::{c_char, c_int, CStr};
    unsafe extern "C" {
        fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
    }
    let read = |name: &CStr| -> Option<String> {
        let mut buf = [0 as c_char; 92]; // PROP_VALUE_MAX
        // SAFETY: `name` termina en NUL y el búfer tiene PROP_VALUE_MAX octetos, el máximo que
        // bionic escribe (con su terminador).
        let n = unsafe { __system_property_get(name.as_ptr(), buf.as_mut_ptr()) };
        if n <= 0 {
            return None;
        }
        // SAFETY: bionic deja la cadena terminada en NUL dentro del búfer.
        Some(unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned())
    };
    read(c"persist.sys.locale").or_else(|| read(c"ro.product.locale")).into_iter().collect()
}

#[cfg(windows)]
fn system_languages() -> Vec<String> {
    const MUI_LANGUAGE_NAME: u32 = 0x8;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserPreferredUILanguages(flags: u32, count: *mut u32, buffer: *mut u16, size: *mut u32) -> i32;
    }
    let (mut count, mut size) = (0u32, 0u32);
    // SAFETY: la primera llamada, con búfer nulo, solo devuelve el tamaño (en `u16`) de la lista
    // doblemente terminada; la segunda escribe como mucho `size` unidades en un búfer de ese tamaño.
    unsafe {
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, std::ptr::null_mut(), &mut size) == 0 || size == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u16; size as usize];
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, buf.as_mut_ptr(), &mut size) == 0 {
            return Vec::new();
        }
        buf.split(|u| *u == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
    }
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android"))))]
fn system_languages() -> Vec<String> {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    // `LANGUAGE` es la lista de preferencia de gettext; después, la primera de las tres que exista.
    let mut out: Vec<String> = var("LANGUAGE").map(|l| l.split(':').map(str::to_string).collect()).unwrap_or_default();
    if let Some(one) = var("LC_ALL").or_else(|| var("LC_MESSAGES")).or_else(|| var("LANG")) {
        out.push(one);
    }
    out
}

#[cfg(not(any(unix, windows)))]
fn system_languages() -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_and_bcp47_spellings_normalize_to_bcp47() {
        assert_eq!(normalize("es_CL.UTF-8").as_deref(), Some("es-CL"));
        assert_eq!(normalize("en-us").as_deref(), Some("en-US"));
        assert_eq!(normalize("ca_ES@valencia").as_deref(), Some("ca-ES"));
        assert_eq!(normalize("zh-hans-cn").as_deref(), Some("zh-Hans-CN"));
        assert_eq!(normalize("ES").as_deref(), Some("es"));
        assert_eq!(normalize("es-419").as_deref(), Some("es-419"));
        for none in ["", "C", "POSIX", "C.UTF-8", "es_", "../x"] {
            assert_eq!(normalize(none), None, "{none:?}");
        }
    }
}
