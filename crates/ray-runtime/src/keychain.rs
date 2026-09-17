//! M266 — `std/keychain` (ray-sublime §96): secretos en el LLAVERO DEL SISTEMA, no en un archivo.
//!
//! Una app de escritorio guarda claves de API, tokens y contraseñas; lo correcto es el almacén
//! que el sistema ya protege con la sesión del usuario, y raylang no lo exponía (la app caía a un
//! `Secrets.json` con `chmod 0600`, que en Windows no significa nada). Tres backends A MANO, cero
//! crates (la línea de `std/audio`/`std/ui`: sin headers de build, sin dependencias nuevas):
//!   - macOS: Keychain Services (`SecItemAdd`/`SecItemCopyMatching`/`SecItemUpdate`/`SecItemDelete`
//!     sobre `kSecClassGenericPassword`; Security + CoreFoundation, frameworks SIEMPRE presentes).
//!   - Linux: Secret Service por `libsecret-1.so.0` EN RUNTIME (`dlopen`; sin la lib o sin demonio
//!     → `Err` claro). El esquema es de dos atributos: `service` y `account`.
//!   - Windows: Credential Manager (`CredWriteW`/`CredReadW`/`CredDeleteW`, advapi32; credencial
//!     genérica con target `service/account`, blob = el secreto en UTF-8).
//!   - `RAY_KEYCHAIN_FILE=<ruta>`: un archivo PLANO (líneas `service\taccount\thex(secreto)`, 0600
//!     en unix) — la vía de los tests y del CI (sin llavero, sin diálogos). NO para producción.
//!
//! Semántica única: `get` → `Ok(Some)`/`Ok(None)`; `set` crea o REEMPLAZA; `delete` → `Ok(true)`
//! si había algo. Los secretos son `string` (UTF-8): lo que guardan las apps son tokens de texto.
#![cfg(all(feature = "keychain", any(unix, windows), not(target_arch = "wasm32")))]

/// Lee el secreto de `(service, account)`; `Ok(None)` si no existe.
pub fn get(service: &str, account: &str) -> Result<Option<String>, String> {
    check(service, account)?;
    if let Some(path) = file_backend() {
        return file::get(&path, service, account);
    }
    native::get(service, account)
}

/// Guarda (crea o reemplaza) el secreto de `(service, account)`.
pub fn set(service: &str, account: &str, secret: &str) -> Result<(), String> {
    check(service, account)?;
    if let Some(path) = file_backend() {
        return file::set(&path, service, account, secret);
    }
    native::set(service, account, secret)
}

/// Borra el secreto de `(service, account)`; `Ok(true)` si existía.
pub fn delete(service: &str, account: &str) -> Result<bool, String> {
    check(service, account)?;
    if let Some(path) = file_backend() {
        return file::delete(&path, service, account);
    }
    native::delete(service, account)
}

/// La forma "arreglo etiquetado" que comparten `builtins.rs` (VM/intérprete) y el nativo emitido:
/// `get` → `["ok", secreto]` / `["none"]`; `set` → `["ok"]`; `delete` → `["ok", "true"|"false"]`;
/// cualquier fallo → `["err", mensaje]`.
pub fn op(op: &str, service: &str, account: &str, secret: &str) -> Vec<String> {
    let err = |e: String| vec!["err".to_string(), e];
    match op {
        "get" => match get(service, account) {
            Ok(Some(v)) => vec!["ok".to_string(), v],
            Ok(None) => vec!["none".to_string()],
            Err(e) => err(e),
        },
        "set" => match set(service, account, secret) {
            Ok(()) => vec!["ok".to_string()],
            Err(e) => err(e),
        },
        "delete" => match delete(service, account) {
            Ok(b) => vec!["ok".to_string(), b.to_string()],
            Err(e) => err(e),
        },
        other => err(format!("keychain: unknown operation '{other}'")),
    }
}

/// Un `service`/`account` vacío o con NUL no tiene sentido en ningún backend (y en el de archivo
/// el separador es el tabulador): se rechaza antes de tocar el llavero.
fn check(service: &str, account: &str) -> Result<(), String> {
    for (what, v) in [("service", service), ("account", account)] {
        if v.is_empty() {
            return Err(format!("keychain: the {what} must not be empty"));
        }
        if v.contains('\0') || v.contains('\t') || v.contains('\n') {
            return Err(format!("keychain: the {what} must not contain NUL, tab or newline"));
        }
    }
    Ok(())
}

fn file_backend() -> Option<String> {
    std::env::var("RAY_KEYCHAIN_FILE").ok().filter(|p| !p.is_empty())
}

// ── Backend de archivo (tests/CI): `service\taccount\thex(secreto)` por línea ────────────────
mod file {
    fn load(path: &str) -> Result<Vec<(String, String, String)>, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("keychain: {e}")),
        };
        let mut out = Vec::new();
        for line in text.lines() {
            let mut it = line.splitn(3, '\t');
            if let (Some(s), Some(a), Some(h)) = (it.next(), it.next(), it.next()) {
                out.push((s.to_string(), a.to_string(), unhex(h)?));
            }
        }
        Ok(out)
    }

    fn save(path: &str, rows: &[(String, String, String)]) -> Result<(), String> {
        let mut text = String::new();
        for (s, a, v) in rows {
            text.push_str(s);
            text.push('\t');
            text.push_str(a);
            text.push('\t');
            text.push_str(&hex(v.as_bytes()));
            text.push('\n');
        }
        std::fs::write(path, text).map_err(|e| format!("keychain: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn get(path: &str, service: &str, account: &str) -> Result<Option<String>, String> {
        Ok(load(path)?.into_iter().find(|(s, a, _)| s == service && a == account).map(|(_, _, v)| v))
    }

    pub fn set(path: &str, service: &str, account: &str, secret: &str) -> Result<(), String> {
        let mut rows = load(path)?;
        rows.retain(|(s, a, _)| !(s == service && a == account));
        rows.push((service.to_string(), account.to_string(), secret.to_string()));
        save(path, &rows)
    }

    pub fn delete(path: &str, service: &str, account: &str) -> Result<bool, String> {
        let mut rows = load(path)?;
        let before = rows.len();
        rows.retain(|(s, a, _)| !(s == service && a == account));
        if rows.len() == before {
            return Ok(false);
        }
        save(path, &rows)?;
        Ok(true)
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn unhex(h: &str) -> Result<String, String> {
        let b = h.as_bytes();
        if b.len() % 2 != 0 {
            return Err("keychain: corrupt store".to_string());
        }
        let mut out = Vec::with_capacity(b.len() / 2);
        for pair in b.chunks(2) {
            let s = std::str::from_utf8(pair).map_err(|_| "keychain: corrupt store".to_string())?;
            out.push(u8::from_str_radix(s, 16).map_err(|_| "keychain: corrupt store".to_string())?);
        }
        String::from_utf8(out).map_err(|_| "keychain: the stored secret is not UTF-8".to_string())
    }
}

// ── macOS: Keychain Services (Security.framework + CoreFoundation, enlazados al build) ────────
#[cfg(target_os = "macos")]
mod native {
    use std::ffi::c_void;

    type CFTypeRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFDataRef = *const c_void;
    type CFDictionaryRef = *const c_void;
    type OSStatus = i32;

    // Solo se toma su DIRECCIÓN: el tamaño declarado no importa mientras el tipo sea Sized.
    #[repr(C)]
    struct CFDictionaryKeyCallBacks {
        _opaque: [usize; 7],
    }
    #[repr(C)]
    struct CFDictionaryValueCallBacks {
        _opaque: [usize; 5],
    }

    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const ERR_SEC_ITEM_NOT_FOUND: OSStatus = -25300;
    const ERR_SEC_DUPLICATE_ITEM: OSStatus = -25299;

    #[link(name = "Security", kind = "framework")]
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kSecClass: CFStringRef;
        static kSecClassGenericPassword: CFStringRef;
        static kSecAttrService: CFStringRef;
        static kSecAttrAccount: CFStringRef;
        static kSecValueData: CFStringRef;
        static kSecReturnData: CFStringRef;
        static kSecMatchLimit: CFStringRef;
        static kSecMatchLimitOne: CFStringRef;
        static kCFBooleanTrue: CFTypeRef;
        static kCFTypeDictionaryKeyCallBacks: CFDictionaryKeyCallBacks;
        static kCFTypeDictionaryValueCallBacks: CFDictionaryValueCallBacks;
        fn SecItemAdd(attributes: CFDictionaryRef, result: *mut CFTypeRef) -> OSStatus;
        fn SecItemCopyMatching(query: CFDictionaryRef, result: *mut CFTypeRef) -> OSStatus;
        fn SecItemUpdate(query: CFDictionaryRef, attributes: CFDictionaryRef) -> OSStatus;
        fn SecItemDelete(query: CFDictionaryRef) -> OSStatus;
        fn SecCopyErrorMessageString(status: OSStatus, reserved: *mut c_void) -> CFStringRef;
        fn CFStringCreateWithBytes(alloc: *const c_void, bytes: *const u8, len: isize, encoding: u32, external: u8) -> CFStringRef;
        fn CFStringGetCString(s: CFStringRef, buf: *mut u8, size: isize, encoding: u32) -> u8;
        fn CFDataCreate(alloc: *const c_void, bytes: *const u8, len: isize) -> CFDataRef;
        fn CFDataGetBytePtr(d: CFDataRef) -> *const u8;
        fn CFDataGetLength(d: CFDataRef) -> isize;
        fn CFDictionaryCreate(
            alloc: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            n: isize,
            key_cb: *const CFDictionaryKeyCallBacks,
            value_cb: *const CFDictionaryValueCallBacks,
        ) -> CFDictionaryRef;
        fn CFRelease(cf: CFTypeRef);
    }

    /// Un objeto CF propio, liberado al soltar (RAII para no fugar en los caminos de error).
    struct Cf(CFTypeRef);
    impl Drop for Cf {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: referencia propia (creada por un `Create`/`Copy`) y no liberada aún.
                unsafe { CFRelease(self.0) };
            }
        }
    }

    fn cf_str(s: &str) -> Result<Cf, String> {
        // SAFETY: los bytes viven durante la llamada; CF copia el contenido.
        let r = unsafe { CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as isize, K_CF_STRING_ENCODING_UTF8, 0) };
        if r.is_null() { Err("keychain: could not build a CFString".to_string()) } else { Ok(Cf(r)) }
    }

    fn cf_data(b: &[u8]) -> Result<Cf, String> {
        // SAFETY: como cf_str; CFData copia los octetos.
        let r = unsafe { CFDataCreate(std::ptr::null(), b.as_ptr(), b.len() as isize) };
        if r.is_null() { Err("keychain: could not build a CFData".to_string()) } else { Ok(Cf(r)) }
    }

    fn cf_dict(pairs: &[(CFTypeRef, CFTypeRef)]) -> Result<Cf, String> {
        let keys: Vec<CFTypeRef> = pairs.iter().map(|p| p.0).collect();
        let values: Vec<CFTypeRef> = pairs.iter().map(|p| p.1).collect();
        // SAFETY: arreglos vivos durante la llamada; los callbacks CF retienen claves y valores,
        // así que los `Cf` del llamador pueden soltarse después.
        let r = unsafe {
            CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                pairs.len() as isize,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            )
        };
        if r.is_null() { Err("keychain: could not build a CFDictionary".to_string()) } else { Ok(Cf(r)) }
    }

    fn describe(status: OSStatus) -> String {
        // SAFETY: `SecCopyErrorMessageString` devuelve una referencia propia (o NULL); el buffer
        // local es del tamaño declarado.
        unsafe {
            let s = SecCopyErrorMessageString(status, std::ptr::null_mut());
            if s.is_null() {
                return format!("keychain: OSStatus {status}");
            }
            let s = Cf(s);
            let mut buf = [0u8; 256];
            if CFStringGetCString(s.0, buf.as_mut_ptr(), buf.len() as isize, K_CF_STRING_ENCODING_UTF8) == 0 {
                return format!("keychain: OSStatus {status}");
            }
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            format!("keychain: {} (OSStatus {status})", String::from_utf8_lossy(&buf[..end]))
        }
    }

    /// El diccionario base `{class: genericPassword, service, account}`.
    fn base(service: &str, account: &str) -> Result<(Cf, Cf, Cf), String> {
        let s = cf_str(service)?;
        let a = cf_str(account)?;
        // SAFETY: lectura de estáticos exportados por Security.framework.
        let d = unsafe {
            cf_dict(&[(kSecClass, kSecClassGenericPassword), (kSecAttrService, s.0), (kSecAttrAccount, a.0)])?
        };
        Ok((d, s, a))
    }

    pub fn get(service: &str, account: &str) -> Result<Option<String>, String> {
        let s = cf_str(service)?;
        let a = cf_str(account)?;
        // SAFETY: estáticos de Security/CoreFoundation; el resultado es una referencia propia.
        unsafe {
            let q = cf_dict(&[
                (kSecClass, kSecClassGenericPassword),
                (kSecAttrService, s.0),
                (kSecAttrAccount, a.0),
                (kSecReturnData, kCFBooleanTrue),
                (kSecMatchLimit, kSecMatchLimitOne),
            ])?;
            let mut out: CFTypeRef = std::ptr::null();
            let st = SecItemCopyMatching(q.0, &mut out);
            if st == ERR_SEC_ITEM_NOT_FOUND {
                return Ok(None);
            }
            if st != 0 {
                return Err(describe(st));
            }
            let data = Cf(out);
            let n = CFDataGetLength(data.0);
            let p = CFDataGetBytePtr(data.0);
            let bytes = if n > 0 && !p.is_null() { std::slice::from_raw_parts(p, n as usize).to_vec() } else { Vec::new() };
            String::from_utf8(bytes).map(Some).map_err(|_| "keychain: the stored secret is not UTF-8".to_string())
        }
    }

    pub fn set(service: &str, account: &str, secret: &str) -> Result<(), String> {
        let (query, _s, _a) = base(service, account)?;
        let v = cf_data(secret.as_bytes())?;
        // SAFETY: estáticos de Security; diccionarios propios vivos durante las llamadas.
        unsafe {
            let attrs = cf_dict(&[(kSecValueData, v.0)])?;
            // Primero UPDATE (reemplaza si existe); si no había, ADD con los atributos + el dato.
            let st = SecItemUpdate(query.0, attrs.0);
            if st == 0 {
                return Ok(());
            }
            if st != ERR_SEC_ITEM_NOT_FOUND {
                return Err(describe(st));
            }
            let s = cf_str(service)?;
            let a = cf_str(account)?;
            let add = cf_dict(&[
                (kSecClass, kSecClassGenericPassword),
                (kSecAttrService, s.0),
                (kSecAttrAccount, a.0),
                (kSecValueData, v.0),
            ])?;
            let st = SecItemAdd(add.0, std::ptr::null_mut());
            if st == 0 || st == ERR_SEC_DUPLICATE_ITEM {
                Ok(())
            } else {
                Err(describe(st))
            }
        }
    }

    pub fn delete(service: &str, account: &str) -> Result<bool, String> {
        let (query, _s, _a) = base(service, account)?;
        // SAFETY: diccionario propio vivo durante la llamada.
        let st = unsafe { SecItemDelete(query.0) };
        match st {
            0 => Ok(true),
            ERR_SEC_ITEM_NOT_FOUND => Ok(false),
            _ => Err(describe(st)),
        }
    }
}

// ── Linux: Secret Service por libsecret (dlopen en runtime; sin la lib → Err claro) ──────────
#[cfg(all(unix, not(target_os = "macos")))]
mod native {
    use std::ffi::{c_char, c_void, CString};
    use std::sync::OnceLock;

    unsafe extern "C" {
        fn dlopen(path: *const c_char, flags: i32) -> *mut c_void;
        fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    }
    const RTLD_NOW: i32 = 2;

    /// `SecretSchemaAttribute { name, type }` (type 0 = STRING).
    #[repr(C)]
    struct SchemaAttribute {
        name: *const c_char,
        kind: i32,
    }
    /// `SecretSchema` tal como lo define secret-schema.h: nombre, flags (0 = NONE), 32 atributos
    /// (terminados por uno con `name` NULL) y los campos reservados a cero.
    #[repr(C)]
    struct Schema {
        name: *const c_char,
        flags: i32,
        attributes: [SchemaAttribute; 32],
        reserved: i32,
        reserved1: *mut c_void,
        reserved2: *mut c_void,
        reserved3: *mut c_void,
        reserved4: *mut c_void,
        reserved5: *mut c_void,
        reserved6: *mut c_void,
        reserved7: *mut c_void,
    }
    // SAFETY (Send/Sync): el esquema es de solo lectura tras construirse; los punteros apuntan a
    // literales estáticos.
    unsafe impl Send for Schema {}
    unsafe impl Sync for Schema {}

    /// `GError { domain: u32, code: i32, message: *mut c_char }`.
    #[repr(C)]
    struct GError {
        domain: u32,
        code: i32,
        message: *mut c_char,
    }

    type FnStore = unsafe extern "C" fn(*const Schema, *const c_char, *const c_char, *const c_char, *mut c_void, *mut *mut GError, ...) -> i32;
    type FnLookup = unsafe extern "C" fn(*const Schema, *mut c_void, *mut *mut GError, ...) -> *mut c_char;
    type FnClear = unsafe extern "C" fn(*const Schema, *mut c_void, *mut *mut GError, ...) -> i32;
    type FnFree = unsafe extern "C" fn(*mut c_char);
    type FnErrorFree = unsafe extern "C" fn(*mut GError);

    struct Lib {
        store: FnStore,
        lookup: FnLookup,
        clear: FnClear,
        free: FnFree,
        error_free: FnErrorFree,
        schema: Schema,
    }
    // SAFETY (Send/Sync): punteros de función resueltos una vez; no hay estado mutable.
    unsafe impl Send for Lib {}
    unsafe impl Sync for Lib {}

    const SERVICE_ATTR: &[u8] = b"service\0";
    const ACCOUNT_ATTR: &[u8] = b"account\0";
    const SCHEMA_NAME: &[u8] = b"dev.raylang.keychain\0";
    const COLLECTION_DEFAULT: &[u8] = b"default\0";

    fn lib() -> Result<&'static Lib, String> {
        static LIB: OnceLock<Result<Lib, String>> = OnceLock::new();
        LIB.get_or_init(load).as_ref().map_err(|e| e.clone())
    }

    fn load() -> Result<Lib, String> {
        // SAFETY: dlopen/dlsym con C-strings NUL-terminados; los símbolos se transmutan a las
        // firmas documentadas de libsecret (funciones variádicas con la lista terminada en NULL).
        unsafe {
            let h = dlopen(c"libsecret-1.so.0".as_ptr(), RTLD_NOW);
            if h.is_null() {
                return Err("keychain: libsecret-1.so.0 is not available (install libsecret and a Secret Service daemon such as gnome-keyring)".to_string());
            }
            let sym = |name: &std::ffi::CStr| -> Result<*mut c_void, String> {
                let p = dlsym(h, name.as_ptr());
                if p.is_null() { Err(format!("keychain: libsecret without {}", name.to_string_lossy())) } else { Ok(p) }
            };
            let store: FnStore = std::mem::transmute(sym(c"secret_password_store_sync")?);
            let lookup: FnLookup = std::mem::transmute(sym(c"secret_password_lookup_sync")?);
            let clear: FnClear = std::mem::transmute(sym(c"secret_password_clear_sync")?);
            let free: FnFree = std::mem::transmute(sym(c"secret_password_free")?);
            let error_free: FnErrorFree = std::mem::transmute(sym(c"g_error_free")?);
            let mut attributes: [SchemaAttribute; 32] = std::array::from_fn(|_| SchemaAttribute { name: std::ptr::null(), kind: 0 });
            attributes[0] = SchemaAttribute { name: SERVICE_ATTR.as_ptr() as *const c_char, kind: 0 };
            attributes[1] = SchemaAttribute { name: ACCOUNT_ATTR.as_ptr() as *const c_char, kind: 0 };
            let schema = Schema {
                name: SCHEMA_NAME.as_ptr() as *const c_char,
                flags: 0,
                attributes,
                reserved: 0,
                reserved1: std::ptr::null_mut(),
                reserved2: std::ptr::null_mut(),
                reserved3: std::ptr::null_mut(),
                reserved4: std::ptr::null_mut(),
                reserved5: std::ptr::null_mut(),
                reserved6: std::ptr::null_mut(),
                reserved7: std::ptr::null_mut(),
            };
            Ok(Lib { store, lookup, clear, free, error_free, schema })
        }
    }

    fn take_error(l: &Lib, err: *mut GError, what: &str) -> String {
        if err.is_null() {
            return format!("keychain: {what} failed");
        }
        // SAFETY: GError propio devuelto por libsecret; se lee el mensaje y se libera una vez.
        unsafe {
            let msg = if (*err).message.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr((*err).message).to_string_lossy().into_owned()
            };
            (l.error_free)(err);
            format!("keychain: {what}: {msg}")
        }
    }

    fn cstr(s: &str) -> Result<CString, String> {
        CString::new(s).map_err(|_| "keychain: NUL in argument".to_string())
    }

    pub fn get(service: &str, account: &str) -> Result<Option<String>, String> {
        let l = lib()?;
        let (s, a) = (cstr(service)?, cstr(account)?);
        let mut err: *mut GError = std::ptr::null_mut();
        // SAFETY: lista variádica `attr, valor, …, NULL`; los CString viven durante la llamada;
        // el resultado se copia y se libera con secret_password_free.
        unsafe {
            let p = (l.lookup)(
                &l.schema,
                std::ptr::null_mut(),
                &mut err,
                SERVICE_ATTR.as_ptr() as *const c_char,
                s.as_ptr(),
                ACCOUNT_ATTR.as_ptr() as *const c_char,
                a.as_ptr(),
                std::ptr::null::<c_char>(),
            );
            if !err.is_null() {
                return Err(take_error(l, err, "lookup"));
            }
            if p.is_null() {
                return Ok(None);
            }
            let v = std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned();
            (l.free)(p);
            Ok(Some(v))
        }
    }

    pub fn set(service: &str, account: &str, secret: &str) -> Result<(), String> {
        let l = lib()?;
        let (s, a, v) = (cstr(service)?, cstr(account)?, cstr(secret)?);
        let label = cstr(&format!("{service} ({account})"))?;
        let mut err: *mut GError = std::ptr::null_mut();
        // SAFETY: como en get; `store_sync` REEMPLAZA el ítem con los mismos atributos.
        let ok = unsafe {
            (l.store)(
                &l.schema,
                COLLECTION_DEFAULT.as_ptr() as *const c_char,
                label.as_ptr(),
                v.as_ptr(),
                std::ptr::null_mut(),
                &mut err,
                SERVICE_ATTR.as_ptr() as *const c_char,
                s.as_ptr(),
                ACCOUNT_ATTR.as_ptr() as *const c_char,
                a.as_ptr(),
                std::ptr::null::<c_char>(),
            )
        };
        if !err.is_null() {
            return Err(take_error(l, err, "store"));
        }
        if ok == 0 { Err("keychain: store failed".to_string()) } else { Ok(()) }
    }

    pub fn delete(service: &str, account: &str) -> Result<bool, String> {
        let l = lib()?;
        let (s, a) = (cstr(service)?, cstr(account)?);
        let mut err: *mut GError = std::ptr::null_mut();
        // SAFETY: como en get. `clear_sync` devuelve TRUE si borró algo.
        let removed = unsafe {
            (l.clear)(
                &l.schema,
                std::ptr::null_mut(),
                &mut err,
                SERVICE_ATTR.as_ptr() as *const c_char,
                s.as_ptr(),
                ACCOUNT_ATTR.as_ptr() as *const c_char,
                a.as_ptr(),
                std::ptr::null::<c_char>(),
            )
        };
        if !err.is_null() {
            return Err(take_error(l, err, "clear"));
        }
        Ok(removed != 0)
    }
}

// ── Windows: Credential Manager (advapi32, siempre presente) ─────────────────────────────────
#[cfg(windows)]
mod native {
    use std::ffi::c_void;

    #[repr(C)]
    struct CredentialW {
        flags: u32,
        kind: u32,
        target_name: *mut u16,
        comment: *mut u16,
        last_written: u64,
        blob_size: u32,
        blob: *mut u8,
        persist: u32,
        attribute_count: u32,
        attributes: *mut c_void,
        target_alias: *mut u16,
        user_name: *mut u16,
    }

    const CRED_TYPE_GENERIC: u32 = 1;
    const CRED_PERSIST_LOCAL_MACHINE: u32 = 2;
    const ERROR_NOT_FOUND: u32 = 1168;

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn CredWriteW(credential: *const CredentialW, flags: u32) -> i32;
        fn CredReadW(target: *const u16, kind: u32, flags: u32, credential: *mut *mut CredentialW) -> i32;
        fn CredDeleteW(target: *const u16, kind: u32, flags: u32) -> i32;
        fn CredFree(buffer: *mut c_void);
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLastError() -> u32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn target(service: &str, account: &str) -> Vec<u16> {
        wide(&format!("{service}/{account}"))
    }

    pub fn get(service: &str, account: &str) -> Result<Option<String>, String> {
        let t = target(service, account);
        let mut p: *mut CredentialW = std::ptr::null_mut();
        // SAFETY: target NUL-terminado vivo durante la llamada; el buffer devuelto se libera con
        // CredFree tras copiar el blob.
        unsafe {
            if CredReadW(t.as_ptr(), CRED_TYPE_GENERIC, 0, &mut p) == 0 {
                let e = GetLastError();
                return if e == ERROR_NOT_FOUND { Ok(None) } else { Err(format!("keychain: CredReadW failed (error {e})")) };
            }
            let n = (*p).blob_size as usize;
            let bytes = if n > 0 && !(*p).blob.is_null() { std::slice::from_raw_parts((*p).blob, n).to_vec() } else { Vec::new() };
            CredFree(p as *mut c_void);
            String::from_utf8(bytes).map(Some).map_err(|_| "keychain: the stored secret is not UTF-8".to_string())
        }
    }

    pub fn set(service: &str, account: &str, secret: &str) -> Result<(), String> {
        let mut t = target(service, account);
        let mut u = wide(account);
        let mut blob = secret.as_bytes().to_vec();
        let cred = CredentialW {
            flags: 0,
            kind: CRED_TYPE_GENERIC,
            target_name: t.as_mut_ptr(),
            comment: std::ptr::null_mut(),
            last_written: 0,
            blob_size: blob.len() as u32,
            blob: blob.as_mut_ptr(),
            persist: CRED_PERSIST_LOCAL_MACHINE,
            attribute_count: 0,
            attributes: std::ptr::null_mut(),
            target_alias: std::ptr::null_mut(),
            user_name: u.as_mut_ptr(),
        };
        // SAFETY: todos los buffers referenciados viven durante la llamada; CredWriteW copia.
        unsafe {
            if CredWriteW(&cred, 0) == 0 {
                return Err(format!("keychain: CredWriteW failed (error {})", GetLastError()));
            }
        }
        Ok(())
    }

    pub fn delete(service: &str, account: &str) -> Result<bool, String> {
        let t = target(service, account);
        // SAFETY: target NUL-terminado vivo durante la llamada.
        unsafe {
            if CredDeleteW(t.as_ptr(), CRED_TYPE_GENERIC, 0) == 0 {
                let e = GetLastError();
                return if e == ERROR_NOT_FOUND { Ok(false) } else { Err(format!("keychain: CredDeleteW failed (error {e})")) };
            }
        }
        Ok(true)
    }
}
