//! M180 (W7d) — `ray bundle` en Windows: el formato del SO es un directorio `<name>\` con
//! `<name>.exe` y un acceso directo `<name>.lnk`. El `.exe` es el binario nativo del programa
//! con tres retoques que Explorer y el Panel de propiedades esperan de una app de escritorio:
//!
//! 1. **Subsistema WINDOWS** (un bit del encabezado PE): al doble clic no se abre una consola
//!    negra detrás de la ventana — como el `.app` de macOS, la app no tiene terminal (`print`
//!    va a la nada). Lanzado desde una consola, tampoco se adjunta a ella.
//! 2. **Icono** como recurso `RT_GROUP_ICON`/`RT_ICON` (Explorer lo pinta en el exe y en el
//!    acceso directo). Un PNG de hasta 256 px va TAL CUAL (Vista+ acepta entradas PNG; es la
//!    convención del tamaño 256); uno mayor se reescala a 256 con System.Drawing vía
//!    PowerShell — el `sips` de aquí (siempre presente, sin crates).
//! 3. **VERSIONINFO** (nombre, versión, copyright de `[app] copyright`): lo que muestra la
//!    pestaña "Detalles" de Propiedades — el `Info.plist` de Windows. El bloque se construye a
//!    mano (es una jerarquía de `{wLength, wValueLength, wType, szKey, value, children}`
//!    alineada a 4 bytes) y se inyecta con `UpdateResourceW` de kernel32, sin `rc.exe`.
//!
//! El `.lnk` lo escribe `WScript.Shell` (COM, vía PowerShell): apunta al exe con ruta ABSOLUTA
//! y cwd = el directorio del bundle (el `Exec=` absoluto del `.desktop` de Linux); para
//! instalarlo, copiarlo al menú Inicio (`%APPDATA%\Microsoft\Windows\Start Menu\Programs`).
//! Sin firma (Authenticode) en v1: SmartScreen avisará de un exe descargado, como Gatekeeper.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Empaqueta `bin` (el binario nativo ya construido) como `<out_dir>\<name>\`. Devuelve el
/// directorio del bundle; los fallos que impiden el bundle son `Err` (el CLI sale 74); el icono
/// y el acceso directo son best-effort con aviso, como el codesign ad-hoc de macOS.
pub fn bundle(out_dir: &Path, name: &str, version: &str, copyright: Option<&str>, icon: Option<&Path>, bin: &Path) -> Result<PathBuf, String> {
    let dir = out_dir.join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| format!("could not create '{}': {e}", dir.display()))?;
    let exe = dir.join(format!("{name}.exe"));
    fs::copy(bin, &exe).map_err(|e| format!("could not place the binary: {e}"))?;
    set_gui_subsystem(&exe)?;

    let mut resources: Vec<(u16, u16, Vec<u8>)> = vec![(RT_VERSION, 1, version_info(name, version, copyright))];
    if let Some(icon) = icon {
        match icon_resources(icon, &dir) {
            Ok((group, image)) => {
                resources.push((RT_ICON, 1, image));
                resources.push((RT_GROUP_ICON, 1, group));
            }
            Err(e) => eprintln!("bundle: warning: could not use the icon ({e}); continuing without it"),
        }
    }
    sys::update_resources(&exe, &resources)?;

    if let Err(e) = write_shortcut(&dir, name, &exe) {
        eprintln!("bundle: warning: could not write the shortcut ({e}); continuing without it");
    }
    println!("ok: bundle '{}'", dir.display());
    Ok(dir)
}

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
const IMAGE_SUBSYSTEM_WINDOWS_GUI: u16 = 2;

/// Marca el PE como app de ventanas: `OptionalHeader.Subsystem` (offset 68 tanto en PE32 como
/// en PE32+) = 2. Verifica las firmas "MZ"/"PE\0\0" antes de tocar nada.
fn set_gui_subsystem(exe: &Path) -> Result<(), String> {
    let mut bytes = fs::read(exe).map_err(|e| format!("could not read the binary: {e}"))?;
    let bad = |what: &str| Err(format!("'{}' is not a PE executable ({what})", exe.display()));
    if bytes.len() < 0x40 || &bytes[..2] != b"MZ" {
        return bad("no MZ header");
    }
    let pe = u32::from_le_bytes([bytes[0x3c], bytes[0x3d], bytes[0x3e], bytes[0x3f]]) as usize;
    if bytes.len() < pe + 24 + 70 || &bytes[pe..pe + 4] != b"PE\0\0" {
        return bad("no PE header");
    }
    let subsystem = pe + 24 + 68;
    bytes[subsystem..subsystem + 2].copy_from_slice(&IMAGE_SUBSYSTEM_WINDOWS_GUI.to_le_bytes());
    fs::write(exe, bytes).map_err(|e| format!("could not write the binary: {e}"))
}

/// El icono: `(GRPICONDIR con una entrada, bytes PNG)`. Lee las dimensiones del IHDR; si el PNG
/// pasa de 256 px (o no es cuadrado) lo reescala a 256×256 con System.Drawing.
fn icon_resources(icon: &Path, work: &Path) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut png = fs::read(icon).map_err(|e| format!("{}: {e}", icon.display()))?;
    let (mut w, mut h) = png_size(&png).ok_or_else(|| format!("'{}' is not a PNG (use a PNG icon, ideally 256x256)", icon.display()))?;
    if w > 256 || h > 256 || w != h {
        let resized = work.join(".icon256.png");
        resize_png(icon, &resized, 256)?;
        png = fs::read(&resized).map_err(|e| format!("resized icon: {e}"))?;
        let _ = fs::remove_file(&resized);
        (w, h) = png_size(&png).ok_or("the resized icon is not a PNG")?;
    }
    // GRPICONDIR (6) + GRPICONDIRENTRY (14): width/height 0 = 256, 32 bpp, id del RT_ICON = 1.
    let mut group = Vec::with_capacity(20);
    group.extend_from_slice(&0u16.to_le_bytes()); // reserved
    group.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    group.extend_from_slice(&1u16.to_le_bytes()); // count
    group.push(if w >= 256 { 0 } else { w as u8 });
    group.push(if h >= 256 { 0 } else { h as u8 });
    group.push(0); // colors (paleta): ninguna
    group.push(0); // reserved
    group.extend_from_slice(&1u16.to_le_bytes()); // planes
    group.extend_from_slice(&32u16.to_le_bytes()); // bits
    group.extend_from_slice(&(png.len() as u32).to_le_bytes());
    group.extend_from_slice(&1u16.to_le_bytes()); // nID
    Ok((group, png))
}

/// Ancho y alto del IHDR de un PNG (firma de 8 bytes + chunk IHDR: longitud, "IHDR", w, h).
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || bytes[..8] != [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a] || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let be = |i: usize| u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    Some((be(16), be(20)))
}

/// `sips -z` de Windows: System.Drawing por PowerShell (bicúbico de alta calidad).
fn resize_png(src: &Path, dst: &Path, size: u32) -> Result<(), String> {
    let script = format!(
        "Add-Type -AssemblyName System.Drawing; \
         $src = [System.Drawing.Image]::FromFile('{}'); \
         $bmp = New-Object System.Drawing.Bitmap {size}, {size}; \
         $g = [System.Drawing.Graphics]::FromImage($bmp); \
         $g.InterpolationMode = 'HighQualityBicubic'; \
         $g.DrawImage($src, 0, 0, {size}, {size}); \
         $bmp.Save('{}', [System.Drawing.Imaging.ImageFormat]::Png); \
         $g.Dispose(); $bmp.Dispose(); $src.Dispose()",
        ps_quote(src),
        ps_quote(dst)
    );
    run_powershell(&script).map_err(|e| format!("could not resize the icon to {size}x{size}: {e}"))
}

/// El acceso directo, por `WScript.Shell` (COM): destino y cwd absolutos, icono = el del exe.
fn write_shortcut(dir: &Path, name: &str, exe: &Path) -> Result<(), String> {
    let abs_dir = plain(&dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf()));
    let abs_exe = plain(&exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf()));
    let lnk = abs_dir.join(format!("{name}.lnk"));
    let script = format!(
        "$s = (New-Object -ComObject WScript.Shell).CreateShortcut('{}'); \
         $s.TargetPath = '{}'; $s.WorkingDirectory = '{}'; $s.IconLocation = '{},0'; \
         $s.Description = '{}'; $s.Save()",
        ps_quote(&lnk),
        ps_quote(&abs_exe),
        ps_quote(&abs_dir),
        ps_quote(&abs_exe),
        name.replace('\'', "''")
    );
    run_powershell(&script)
}

fn run_powershell(script: &str) -> Result<(), String> {
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .output()
        .map_err(|e| format!("powershell: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(err.lines().next().unwrap_or("powershell failed").trim().to_string())
    }
}

/// `canonicalize` devuelve rutas `\\?\C:\…`; WScript y System.Drawing quieren la forma llana.
fn plain(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
}

/// Ruta entre comillas simples de PowerShell (`'` se dobla).
fn ps_quote(p: &Path) -> String {
    plain(p).to_string_lossy().replace('\'', "''")
}

/// El bloque VS_VERSIONINFO completo: raíz con VS_FIXEDFILEINFO + StringFileInfo (tabla
/// `040904B0`: inglés-US, Unicode) + VarFileInfo/Translation. Versión `a.b.c[.d]` (los sufijos
/// no numéricos se ignoran: "1.6.0-beta" → 1.6.0.0).
fn version_info(name: &str, version: &str, copyright: Option<&str>) -> Vec<u8> {
    let parts: Vec<u16> = version
        .split('.')
        .map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse::<u16>().unwrap_or(0))
        .chain(std::iter::repeat(0))
        .take(4)
        .collect();
    let ms = ((parts[0] as u32) << 16) | parts[1] as u32;
    let ls = ((parts[2] as u32) << 16) | parts[3] as u32;
    let mut fixed = Vec::with_capacity(52);
    for dword in [0xFEEF_04BDu32, 0x0001_0000, ms, ls, ms, ls, 0x3F, 0, 0x0004_0004, 1, 0, 0, 0] {
        fixed.extend_from_slice(&dword.to_le_bytes());
    }
    let file_version = format!("{}.{}.{}.{}", parts[0], parts[1], parts[2], parts[3]);
    let mut strings = vec![
        ("FileDescription", name.to_string()),
        ("FileVersion", file_version.clone()),
        ("InternalName", name.to_string()),
        ("OriginalFilename", format!("{name}.exe")),
        ("ProductName", name.to_string()),
        ("ProductVersion", version.to_string()),
    ];
    if let Some(c) = copyright.filter(|c| !c.is_empty()) {
        strings.push(("LegalCopyright", c.to_string()));
    }
    let table: Vec<Vec<u8>> = strings.iter().map(|(k, v)| block(k, Value::Text(v), &[])).collect();
    let string_table = block("040904B0", Value::None, &table);
    let string_file_info = block("StringFileInfo", Value::None, &[string_table]);
    let translation = block("Translation", Value::Binary(&0x04B0_0409u32.to_le_bytes()), &[]);
    let var_file_info = block("VarFileInfo", Value::None, &[translation]);
    block("VS_VERSION_INFO", Value::Binary(&fixed), &[string_file_info, var_file_info])
}

enum Value<'a> {
    None,
    Text(&'a str),
    Binary(&'a [u8]),
}

/// Un nodo del árbol de VERSIONINFO: `wLength, wValueLength, wType, szKey\0, pad, value, pad,
/// children`. `wValueLength` cuenta WCHARs (con el nul) si es texto y bytes si es binario; el
/// nodo va relleno a múltiplo de 4 y `wLength` incluye el relleno (el recorrido de
/// `VerQueryValue` alinea igual).
fn block(key: &str, value: Value, children: &[Vec<u8>]) -> Vec<u8> {
    let (value_len, kind, bytes): (u16, u16, Vec<u8>) = match value {
        Value::None => (0, 1, Vec::new()),
        Value::Text(t) => {
            let w = wstr(t);
            ((w.len() / 2) as u16, 1, w)
        }
        Value::Binary(b) => (b.len() as u16, 0, b.to_vec()),
    };
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // wLength (se rellena al final)
    out.extend_from_slice(&value_len.to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&wstr(key));
    pad4(&mut out);
    out.extend_from_slice(&bytes);
    pad4(&mut out);
    for child in children {
        out.extend_from_slice(child);
    }
    let len = out.len() as u16;
    out[..2].copy_from_slice(&len.to_le_bytes());
    out
}

fn wstr(s: &str) -> Vec<u8> {
    s.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect()
}

fn pad4(v: &mut Vec<u8>) {
    while v.len() % 4 != 0 {
        v.push(0);
    }
}

/// kernel32: la API de recursos de PE. Un `unsafe` mínimo, anotado en SECURITY.md.
mod sys {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn BeginUpdateResourceW(file_name: *const u16, delete_existing: i32) -> *mut c_void;
        fn UpdateResourceW(update: *mut c_void, kind: *const u16, name: *const u16, language: u16, data: *const c_void, size: u32) -> i32;
        fn EndUpdateResourceW(update: *mut c_void, discard: i32) -> i32;
    }

    const LANG_EN_US: u16 = 0x0409;

    /// Inyecta `(tipo, id, datos)` en el exe. Tipo e id son enteros (`MAKEINTRESOURCE`: el
    /// puntero ES el número). Todo o nada: un fallo descarta la sesión sin tocar el archivo.
    pub fn update_resources(exe: &Path, resources: &[(u16, u16, Vec<u8>)]) -> Result<(), String> {
        let path: Vec<u16> = exe.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        // SAFETY: `path` es NUL-terminada y vive durante la llamada; los datos de cada recurso
        // viven hasta `EndUpdateResourceW`, que los copia al archivo.
        unsafe {
            let h = BeginUpdateResourceW(path.as_ptr(), 0);
            if h.is_null() {
                return Err(format!("could not open the binary for resource update: {}", std::io::Error::last_os_error()));
            }
            for (kind, id, data) in resources {
                if UpdateResourceW(h, *kind as usize as *const u16, *id as usize as *const u16, LANG_EN_US, data.as_ptr().cast(), data.len() as u32) == 0 {
                    let e = std::io::Error::last_os_error();
                    EndUpdateResourceW(h, 1);
                    return Err(format!("could not add resource type {kind}: {e}"));
                }
            }
            if EndUpdateResourceW(h, 0) == 0 {
                return Err(format!("could not write the resources: {}", std::io::Error::last_os_error()));
            }
        }
        Ok(())
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_info_is_a_well_formed_tree() {
        let v = version_info("mini-app", "2.5.0-beta", Some("© 2026 Ray"));
        assert_eq!(v.len() % 4, 0, "relleno a 4");
        assert_eq!(u16::from_le_bytes([v[0], v[1]]) as usize, v.len(), "wLength de la raíz = todo");
        assert_eq!(u16::from_le_bytes([v[2], v[3]]), 52, "wValueLength = VS_FIXEDFILEINFO");
        let key: Vec<u8> = wstr("VS_VERSION_INFO");
        assert_eq!(&v[6..6 + key.len()], &key[..]);
        let fixed_at = (6 + key.len() + 3) & !3;
        assert_eq!(&v[fixed_at..fixed_at + 4], &0xFEEF_04BDu32.to_le_bytes(), "firma del fixed info");
        // 2.5.0-beta → 2.5.0.0: MS = 2<<16 | 5, LS = 0.
        assert_eq!(&v[fixed_at + 8..fixed_at + 12], &((2u32 << 16) | 5).to_le_bytes());
        assert_eq!(&v[fixed_at + 12..fixed_at + 16], &0u32.to_le_bytes());
        let text = String::from_utf16_lossy(&v.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>());
        for needle in ["StringFileInfo", "040904B0", "ProductVersion", "2.5.0-beta", "LegalCopyright", "© 2026 Ray", "VarFileInfo", "Translation"] {
            assert!(text.contains(needle), "contiene {needle}");
        }
    }

    #[test]
    fn png_size_reads_the_ihdr() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&512u32.to_be_bytes());
        png.extend_from_slice(&128u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((512, 128)));
        assert_eq!(png_size(b"not a png at all, really not"), None);
    }

    #[test]
    fn the_subsystem_patch_checks_the_signatures() {
        let dir = std::env::temp_dir().join("ray_bundle_windows_pe");
        let _ = fs::create_dir_all(&dir);
        let bogus = dir.join("bogus.exe");
        fs::write(&bogus, b"MZ nope").unwrap();
        assert!(set_gui_subsystem(&bogus).unwrap_err().contains("not a PE executable"));
        let _ = fs::remove_dir_all(&dir);
    }
}
