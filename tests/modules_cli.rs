//! Pruebas de módulos (M11.3a) sobre el binario: escriben varios archivos `.ray` en un
//! directorio temporal, ejecutan `raylang <entrada>` y comprueban la salida / código de salida.
//! Los módulos viven en archivos separados, así que se prueban de verdad por subproceso.

use std::io::Write;
use std::process::Command;

/// Crea un directorio temporal único, escribe los `(nombre, fuente)` dados, ejecuta
/// `raylang [--vm] <dir>/<entry>.ray` y devuelve `(stdout, código)`.
fn run_modules(dir: &str, entry: &str, files: &[(&str, &str)], vm: bool) -> (String, i32) {
    let mut base = std::env::temp_dir();
    base.push(dir);
    std::fs::create_dir_all(&base).expect("crea el dir temporal");
    for (name, src) in files {
        let mut path = base.clone();
        path.push(format!("{name}.ray"));
        // El nombre puede ser una ruta con `/` (módulos por directorios, M11.5): crea los padres.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("crea el subdirectorio del módulo");
        }
        let mut f = std::fs::File::create(&path).expect("crea el módulo");
        f.write_all(src.as_bytes()).expect("escribe el módulo");
    }
    let mut entry_path = base.clone();
    entry_path.push(format!("{entry}.ray"));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_raylang"));
    if vm {
        cmd.arg("--vm");
    }
    let out = cmd.arg(&entry_path).output().expect("ejecuta el binario");
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn import_calificado_en_ambos_motores() {
    let files = &[
        ("mates", "pub fn doble(n: int) -> int { n + n }\nfn secreto() -> int { 99 }\n"),
        ("app", "import mates;\nfn doble(x: int) -> int { x + 1 }\nfn main() -> int { print(mates.doble(10)); print(doble(10)); mates.doble(21) }\n"),
    ];
    for vm in [false, true] {
        let (out, code) = run_modules("ray_mod_basico", "app", files, vm);
        assert!(out.contains("20"), "mates.doble(10)=20 (vm={vm})\n{out}");
        assert!(out.contains("11"), "la local doble(10)=11 no colisiona (vm={vm})\n{out}");
        assert_eq!(code, 42, "exit = mates.doble(21) (vm={vm})");
    }
}

#[test]
fn const_calificado_de_modulo() {
    // M49.1c: un `pub const` de un módulo se accede CALIFICADO (`M.CONST`), en ambos motores.
    let files = &[
        ("fisica", "pub const G: float = 9.81;\n"),
        ("app", "import fisica;\nfn main() -> int { print(fisica.G); if (fisica.G > 9.0) { 0 } else { 1 } }\n"),
    ];
    for vm in [false, true] {
        let (out, code) = run_modules("m49_const", "app", files, vm);
        assert_eq!(code, 0, "sale 0 (vm={vm})");
        assert!(out.contains("9.81"), "imprime el const calificado (vm={vm}): {out}");
    }
    // Encapsulación: `G` sin calificar NO filtra al ámbito global (aunque se importe `fisica`).
    let bare = &[
        ("fisica", "pub const G: float = 9.81;\n"),
        ("app", "import fisica;\nfn main() -> int { print(G); 0 }\n"),
    ];
    let (_o, code) = run_modules("m49_const_bare", "app", bare, true);
    assert_ne!(code, 0, "bare `G` no debe resolver (const encapsulado en su módulo)");
    // Un `const` NO-`pub` no es accesible ni calificado.
    let privado = &[
        ("fisica", "const SECRETO: int = 42;\n"),
        ("app", "import fisica;\nfn main() -> int { fisica.SECRETO }\n"),
    ];
    let (_o2, code2) = run_modules("m49_const_priv", "app", privado, true);
    assert_ne!(code2, 0, "un const no-pub no es accesible calificado");
}

#[test]
fn import_transitivo() {
    let files = &[
        ("base", "pub fn uno() -> int { 1 }\n"),
        ("mid", "import base;\nfn interno(n: int) -> int { n + base.uno() }\npub fn cinco() -> int { interno(base.uno()) + interno(2) }\n"),
        ("top", "import mid;\nfn main() -> int { mid.cinco() + 37 }\n"),
    ];
    let (_, code) = run_modules("ray_mod_trans", "top", files, false);
    assert_eq!(code, 42, "5 + 37 = 42 (mid usa base, transitivo)");
}

#[test]
fn from_import_con_alias_en_ambos_motores() {
    // `from M import a as b` trae funciones `pub` al ámbito (sin calificar), con alias para
    // evitar colisiones con una función propia homónima (M11.3b).
    let files = &[
        ("mates", "pub fn doble(n: int) -> int { n + n }\npub fn triple(n: int) -> int { n + n + n }\n"),
        ("app", "from mates import doble as md, triple as tri;\nfn doble(x: int) -> int { x + 1 }\nfn main() -> int { print(doble(10)); print(md(10)); print(tri(10)); md(10) + tri(10) }\n"),
    ];
    for vm in [false, true] {
        let (out, code) = run_modules("ray_from_alias", "app", files, vm);
        assert!(out.contains("11"), "la local doble(10)=11 (vm={vm})\n{out}");
        assert!(out.contains("20"), "md = mates.doble, md(10)=20 (vm={vm})\n{out}");
        assert!(out.contains("30"), "tri = mates.triple, tri(10)=30 (vm={vm})\n{out}");
        assert_eq!(code, 50, "exit = md(10) + tri(10) = 50 (vm={vm})");
    }
}

#[test]
fn from_import_colision_sin_alias_es_error() {
    // Importar un nombre que ya existe en el módulo (sin `as`) es error: pide renombrar.
    let files = &[
        ("mates", "pub fn doble(n: int) -> int { n + n }\n"),
        ("app", "from mates import doble;\nfn doble(x: int) -> int { x + 1 }\nfn main() -> int { doble(1) }\n"),
    ];
    let (_, code) = run_modules("ray_from_colision", "app", files, false);
    assert_ne!(code, 0, "una colisión de nombre importado sin alias debe fallar");
}

#[test]
fn from_import_tipo_pub_en_ambos_motores() {
    // M11.3c-2: `from M import Tipo` trae un tipo `pub` al ámbito (sin calificar). Aquí un struct y
    // un enum; se usan en anotación, literal, construcción de variante y `match`.
    let files = &[
        ("geo", "pub struct Punto { x: int, y: int }\npub enum Eje { X, Y }\n"),
        ("app", "from geo import Punto, Eje;\nfn coord(p: Punto, e: Eje) -> int {\n  match (e) { Eje.X => p.x, Eje.Y => p.y, }\n}\nfn main() -> int {\n  let p: Punto = Punto { x: 11, y: 31 };\n  coord(p, Eje.X) + coord(p, Eje.Y)\n}\n"),
    ];
    for vm in [false, true] {
        let (_, code) = run_modules("ray_from_tipo_pub", "app", files, vm);
        assert_eq!(code, 42, "11 + 31 = 42 con tipos importados (vm={vm})");
    }
}

#[test]
fn from_import_tipo_con_alias() {
    // `from M import Tipo as T` evita la colisión con un tipo propio homónimo.
    let files = &[
        ("geo", "pub struct Punto { x: int, y: int }\n"),
        ("app", "from geo import Punto as P;\nstruct Punto { otro: int }\nfn main() -> int {\n  let a: P = P { x: 40, y: 2 };\n  let b: Punto = Punto { otro: 0 };\n  a.x + a.y + b.otro\n}\n"),
    ];
    for vm in [false, true] {
        let (_, code) = run_modules("ray_from_tipo_alias", "app", files, vm);
        assert_eq!(code, 42, "P (geo.Punto) y Punto (propio) coexisten (vm={vm})");
    }
}

#[test]
fn from_import_tipo_privado_es_error() {
    // Importar un tipo NO `pub` con `from` falla: pide `pub` (encapsulamiento).
    let files = &[
        ("lib", "pub fn f() -> int { 1 }\nstruct Punto { x: int, y: int }\n"),
        ("app", "from lib import Punto;\nfn main() -> int { 0 }\n"),
    ];
    let (_, code) = run_modules("ray_from_tipo_priv", "app", files, false);
    assert_ne!(code, 0, "importar un tipo privado con 'from' debe fallar");
}

#[test]
fn referencia_calificada_M_tipo_en_ambos_motores() {
    // M11.3c-3: `M.Tipo` calificado en las cuatro posiciones — anotación, literal de struct,
    // construcción de enum y patrón de match.
    let files = &[
        ("geo", "pub struct Punto { x: int, y: int }\npub enum Color { Rojo, Verde(int) }\n"),
        ("app", "import geo;\nfn dist(p: geo.Punto) -> int { p.x + p.y }\nfn valor(c: geo.Color) -> int {\n  match (c) { geo.Color.Rojo => 1, geo.Color.Verde(n) => n, }\n}\nfn main() -> int {\n  let p: geo.Punto = geo.Punto { x: 10, y: 5 };\n  print(valor(geo.Color.Rojo));\n  dist(p) + valor(geo.Color.Verde(27))\n}\n"),
    ];
    for vm in [false, true] {
        let (out, code) = run_modules("ray_calif_tipo", "app", files, vm);
        assert!(out.contains("1"), "geo.Color.Rojo => 1 (vm={vm})\n{out}");
        assert_eq!(code, 42, "dist(15) + Verde(27) = 42 (vm={vm})");
    }
}

#[test]
fn referencia_calificada_a_tipo_privado_es_error() {
    // `M.Tipo` con un tipo NO `pub` no resuelve: el checker lo rechaza (encapsulamiento).
    let files = &[
        ("geo", "pub struct Punto { x: int, y: int }\nstruct Secreto { z: int }\n"),
        ("app", "import geo;\nfn main() -> int { let s: geo.Secreto = geo.Secreto { z: 1 }; s.z }\n"),
    ];
    let (_, code) = run_modules("ray_calif_priv", "app", files, false);
    assert_ne!(code, 0, "un tipo privado calificado debe fallar");
}

#[test]
fn referencia_calificada_a_enum_privado_es_error() {
    // Construcción de enum calificada `M.Color.Rojo` con un enum NO `pub` → error de carga.
    let files = &[
        ("geo", "enum Color { Rojo, Verde }\npub fn f() -> int { 0 }\n"),
        ("app", "import geo;\nfn main() -> int { match (geo.Color.Rojo) { geo.Color.Rojo => 1, geo.Color.Verde => 2, } }\n"),
    ];
    let (_, code) = run_modules("ray_calif_enum_priv", "app", files, false);
    assert_ne!(code, 0, "un enum privado calificado debe fallar");
}

#[test]
fn from_import_nombre_inexistente_es_error() {
    let files = &[
        ("lib", "pub fn f() -> int { 1 }\n"),
        ("app", "from lib import NoExiste;\nfn main() -> int { 0 }\n"),
    ];
    let (_, code) = run_modules("ray_from_inexistente", "app", files, false);
    assert_ne!(code, 0, "importar un nombre inexistente debe fallar");
}

#[test]
fn tipos_por_modulo_reusan_nombre() {
    // Dos módulos definen `struct Node` y `enum Estado` (distintos) sin colisionar: los tipos se
    // namespacan por módulo (M11.3c). Cada uno usa los suyos (incl. @derive, match, construcción).
    let files = &[
        ("a", "@derive(Show)\nstruct Node { v: int }\nenum Estado { On, Off }\npub fn f() -> int {\n  let n = Node { v: 7 };\n  let e = Estado.On;\n  print(n.show());\n  match (e) { Estado.On => n.v + 100, Estado.Off => 0, }\n}\n"),
        ("main", "import a;\nstruct Node { propio: bool }\nfn main() -> int {\n  let n = Node { propio: true };\n  a.f()\n}\n"),
    ];
    for vm in [false, true] {
        let (out, code) = run_modules("ray_tipos_modulo", "main", files, vm);
        assert!(out.contains("Node { v: 7 }"), "@derive(Show) en módulo no-entrada usa el nombre local (vm={vm})\n{out}");
        assert_eq!(code, 107, "7 + 100 = 107 (vm={vm})");
    }
}

#[test]
fn tipo_de_otro_modulo_sin_importar_es_error() {
    // Un tipo es privado a su módulo: referenciarlo bare desde otro (sin importar) no resuelve.
    let files = &[
        ("lib", "pub struct Punto { x: int, y: int }\n"),
        ("uso", "import lib;\nfn main() -> int { let p: Punto = Punto { x: 1, y: 2 }; p.x }\n"),
    ];
    let (_, code) = run_modules("ray_tipo_encaps", "uso", files, false);
    assert_ne!(code, 0, "referenciar un tipo de otro módulo sin importarlo debe fallar");
}

#[test]
fn tipo_duplicado_en_un_modulo_es_error() {
    let files = &[
        ("dup", "struct Foo { a: int }\nstruct Foo { b: int }\n"),
        ("main", "import dup;\nfn main() -> int { 0 }\n"),
    ];
    let (_, code) = run_modules("ray_tipo_dup", "main", files, false);
    assert_ne!(code, 0, "dos tipos homónimos en un módulo es error");
}

#[test]
fn colision_de_posiciones_entre_modulos() {
    // Dos llamadas a método en la MISMA (línea, col) de módulos distintos: antes colisionaban en
    // el lowering por posición de M9 y el programa crasheaba en ambos motores. L3 las desambigua
    // dando a cada módulo una banda de líneas distinta. Ambas llamadas caen en (línea 5, col 1).
    let files = &[
        ("m1", "struct A { v: int }\ntrait T { fn dup(self) -> int; }\nimpl T for A { fn dup(self) -> int { self.v + self.v } }\npub fn run_a() -> int {\nA { v: 10 }.dup()\n}\n"),
        ("main", "import m1;\nstruct B { w: int }\ntrait U { fn dup(self) -> int; }\nimpl U for B { fn dup(self) -> int { self.w } } fn run_b() -> int {\nB { w: 7 }.dup()\n}\nfn main() -> int { m1.run_a() + run_b() }\n"),
    ];
    for vm in [false, true] {
        let (_, code) = run_modules("ray_colision_pos", "main", files, vm);
        assert_eq!(code, 27, "20 + 7 = 27 sin colisión de posiciones (vm={vm})");
    }
}

#[test]
fn error_en_modulo_no_entrada_se_atribuye() {
    // Un error de tipos en un módulo importado debe renderizarse contra ESE archivo, con su línea
    // LOCAL (2, no la global) y prefijado con `[mates]`.
    let mut base = std::env::temp_dir();
    base.push("ray_attr_err");
    std::fs::create_dir_all(&base).unwrap();
    for (name, src) in [
        ("mates", "pub fn doble(n: int) -> int {\n  n + \"x\"\n}\n"),
        ("app", "import mates;\nfn main() -> int { mates.doble(5) }\n"),
    ] {
        let mut p = base.clone();
        p.push(format!("{name}.ray"));
        std::fs::File::create(&p).unwrap().write_all(src.as_bytes()).unwrap();
    }
    let mut entry = base.clone();
    entry.push("app.ray");
    let out = Command::new(env!("CARGO_BIN_EXE_raylang")).arg(&entry).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_ne!(out.status.code(), Some(0), "el error de tipos debe fallar");
    assert!(err.contains("[mates]"), "atribuido al módulo mates\n{err}");
    assert!(err.contains("en 2:"), "con su línea local (2), no la global\n{err}");
    // M33a-2: el span de la expresión viaja con la banda de líneas del módulo — el
    // subrayado cubre `n + "x"` entero (7 chars). Si el shift de la tabla se rompiera,
    // la clave no casaría y se dibujaría un solo `^`.
    assert!(err.contains("^^^^^^^"), "subraya la expresión completa\n{err}");
}

#[test]
fn llamar_funcion_privada_es_error() {
    let files = &[
        ("lib", "pub fn publica() -> int { 1 }\nfn privada() -> int { 2 }\n"),
        ("app", "import lib;\nfn main() -> int { lib.privada() }\n"),
    ];
    let (out, code) = run_modules("ray_mod_priv", "app", files, false);
    assert_ne!(code, 0, "una llamada a función privada debe fallar");
    let _ = out;
}

// --- M11.5: módulos por directorios (import a/b/c) -------------------------------------------

#[test]
fn import_por_directorio_funcion_y_tipo() {
    // `import geo/formas/circulo;` resuelve <raíz>/geo/formas/circulo.ray; el leaf es `circulo`,
    // y se accede calificado tanto a una función `pub` como a un tipo `pub`.
    let files = &[
        ("geo/formas/circulo",
         "pub struct Punto { x: int, y: int }\npub fn area(r: int) -> int { 3 * r * r }\n"),
        ("app",
         "import geo/formas/circulo;\nfn main() -> int {\n  let p = circulo.Punto { x: 2, y: 5 };\n  print(p.x);\n  circulo.area(4)\n}\n"),
    ];
    for vm in [false, true] {
        let (out, code) = run_modules("ray_dir_basico", "app", files, vm);
        assert!(out.contains("2"), "campo del tipo importado por ruta (vm={vm})\n{out}");
        assert_eq!(code, 48, "exit = circulo.area(4) = 3*16 = 48 (vm={vm})");
    }
}

#[test]
fn import_por_directorio_con_alias() {
    let files = &[
        ("geo/formas/circulo", "pub fn area(r: int) -> int { 3 * r * r }\n"),
        ("app", "import geo/formas/circulo as c;\nfn main() -> int { c.area(4) }\n"),
    ];
    let (_, code) = run_modules("ray_dir_alias", "app", files, false);
    assert_eq!(code, 48, "el alias `c` accede al módulo por ruta");
}

#[test]
fn import_por_directorio_transitivo_desde_la_raiz() {
    // Un submódulo importa a otro **por su ruta absoluta desde la raíz** (no relativa).
    let files = &[
        ("geo/util", "pub fn cuadrado(n: int) -> int { n * n }\n"),
        ("geo/formas/circulo",
         "import geo/util;\npub fn area(r: int) -> int { 3 * util.cuadrado(r) }\n"),
        ("app", "import geo/formas/circulo;\nfn main() -> int { circulo.area(4) }\n"),
    ];
    let (_, code) = run_modules("ray_dir_trans", "app", files, false);
    assert_eq!(code, 48, "circulo (en geo/formas) usa geo/util por ruta absoluta");
}

#[test]
fn colision_de_leaf_pide_alias() {
    // Dos rutas con el mismo último segmento (`circulo`) colisionan: el segundo necesita `as`.
    let files = &[
        ("a/circulo", "pub fn area(r: int) -> int { r }\n"),
        ("b/circulo", "pub fn area(r: int) -> int { r + r }\n"),
        ("app", "import a/circulo;\nimport b/circulo;\nfn main() -> int { circulo.area(1) }\n"),
    ];
    let (out, code) = run_modules("ray_dir_colision", "app", files, false);
    assert_ne!(code, 0, "la colisión de leaf debe fallar\n{out}");
}

#[test]
fn from_import_por_directorio() {
    let files = &[
        ("geo/formas/circulo", "pub fn area(r: int) -> int { 3 * r * r }\n"),
        ("app", "from geo/formas/circulo import area;\nfn main() -> int { area(4) }\n"),
    ];
    let (_, code) = run_modules("ray_dir_from", "app", files, false);
    assert_eq!(code, 48, "from a/b/c import trae la función sin calificar");
}

// --- M11.6a: cápsula direccionable (mod.ray) + reexport (pub from) ----------------------------

#[test]
fn mod_ray_direccionable_con_reexport() {
    // `import geo;` carga geo/mod.ray, que reexporta una función y un tipo `pub` de un submódulo.
    let files = &[
        ("geo/formas/circulo",
         "pub struct Circulo { radio: int }\npub fn area(c: Circulo) -> int { 3 * c.radio * c.radio }\n"),
        ("geo/mod", "pub from geo/formas/circulo import Circulo, area;\n"),
        ("app",
         "import geo;\nfn main() -> int {\n  let c = geo.Circulo { radio: 4 };\n  geo.area(c)\n}\n"),
    ];
    for vm in [false, true] {
        let (_, code) = run_modules("ray_modray_basico", "app", files, vm);
        assert_eq!(code, 48, "geo.area(Circulo{{radio:4}}) = 48 vía la fachada (vm={vm})");
    }
}

#[test]
fn from_capsula_trae_los_reexportados() {
    // Un `from geo import …` trae al ámbito lo que la cápsula reexporta (sin calificar).
    let files = &[
        ("geo/formas/circulo", "pub fn area(r: int) -> int { 3 * r * r }\n"),
        ("geo/mod", "pub from geo/formas/circulo import area;\n"),
        ("app", "from geo import area;\nfn main() -> int { area(4) }\n"),
    ];
    let (_, code) = run_modules("ray_modray_from", "app", files, false);
    assert_eq!(code, 48, "from geo import area (reexportado) = 48");
}

#[test]
fn reexport_en_cadena_entre_capsulas() {
    // Reexport de un reexport: a/mod reexporta de a/b (que a su vez reexporta de a/b/leaf).
    let files = &[
        ("a/b/leaf", "pub fn val() -> int { 7 }\n"),
        ("a/b/mod", "pub from a/b/leaf import val;\n"),
        ("a/mod", "pub from a/b import val;\n"),
        ("app", "import a;\nfn main() -> int { a.val() + 35 }\n"),
    ];
    for vm in [false, true] {
        let (_, code) = run_modules("ray_modray_cadena", "app", files, vm);
        assert_eq!(code, 42, "7 + 35 = 42 vía reexport en cadena (vm={vm})");
    }
}

#[test]
fn modulo_archivo_y_directorio_homonimos_es_ambiguo() {
    // Una sola forma canónica: si existen geo.ray Y geo/mod.ray, el módulo `geo` es ambiguo.
    let files = &[
        ("geo", "pub fn f() -> int { 1 }\n"),
        ("geo/mod", "pub fn f() -> int { 2 }\n"),
        ("app", "import geo;\nfn main() -> int { geo.f() }\n"),
    ];
    let (_, code) = run_modules("ray_modray_ambiguo", "app", files, false);
    assert_ne!(code, 0, "geo.ray + geo/mod.ray a la vez debe fallar (ambiguo)");
}

// --- M11.6b: enforcement de la cápsula -------------------------------------------------------

#[test]
fn import_directo_de_submodulo_interno_es_error() {
    // Desde fuera de la cápsula, importar un submódulo interno por ruta es ilegal: hay que
    // pasar por la fachada `import geo;`.
    let files = &[
        ("geo/formas/circulo", "pub fn area(r: int) -> int { 3 * r * r }\n"),
        ("geo/mod", "pub from geo/formas/circulo import area;\n"),
        ("app", "import geo/formas/circulo;\nfn main() -> int { circulo.area(4) }\n"),
    ];
    let (out, code) = run_modules("ray_cap_enf_dir", "app", files, false);
    assert_ne!(code, 0, "import directo de un submódulo interno debe fallar\n{out}");
}

#[test]
fn from_de_submodulo_interno_es_error() {
    // Lo mismo con `from`: la arista cruza el borde de la cápsula.
    let files = &[
        ("geo/formas/circulo", "pub fn area(r: int) -> int { 3 * r * r }\n"),
        ("geo/mod", "pub from geo/formas/circulo import area;\n"),
        ("app", "from geo/formas/circulo import area;\nfn main() -> int { area(4) }\n"),
    ];
    let (_, code) = run_modules("ray_cap_enf_from", "app", files, false);
    assert_ne!(code, 0, "from de un submódulo interno desde fuera debe fallar");
}

#[test]
fn acceso_interno_a_la_capsula_sigue_permitido() {
    // Dentro de la cápsula, los submódulos se importan entre sí por ruta sin restricción.
    let files = &[
        ("geo/util", "pub fn cuadrado(n: int) -> int { n * n }\n"),
        ("geo/formas/circulo",
         "import geo/util;\npub fn area(r: int) -> int { 3 * util.cuadrado(r) }\n"),
        ("geo/mod", "pub from geo/formas/circulo import area;\n"),
        ("app", "import geo;\nfn main() -> int { geo.area(4) }\n"),
    ];
    for vm in [false, true] {
        let (_, code) = run_modules("ray_cap_interno_ok", "app", files, vm);
        assert_eq!(code, 48, "acceso interno (circulo usa util) permitido (vm={vm})");
    }
}

#[test]
fn capsulas_anidadas_respetan_el_borde_mas_cercano() {
    // `a/b` es una cápsula dentro de `a` (también cápsula). Un módulo que vive en `a` pero NO en
    // `a/b` no puede alcanzar el interior de `a/b`.
    let files = &[
        ("a/b/leaf", "pub fn val() -> int { 7 }\n"),
        ("a/b/mod", "pub from a/b/leaf import val;\n"),
        // a/otro está dentro de `a` pero fuera de `a/b`: importar a/b/leaf es ilegal.
        ("a/otro", "import a/b/leaf;\npub fn f() -> int { leaf.val() }\n"),
        ("a/mod", "pub from a/otro import f;\n"),
        ("app", "import a;\nfn main() -> int { a.f() }\n"),
    ];
    let (out, code) = run_modules("ray_cap_anidada", "app", files, false);
    assert_ne!(code, 0, "a/otro no puede entrar al interior de la cápsula a/b\n{out}");
}

#[test]
fn ufcs_resuelve_funcion_importada() {
    // UFCS (`recv.f(...)`) debe resolver una función traída por `from M import f` —no solo las del
    // módulo de entrada—. El loader deja el alias local→global (`Program.ufcs_aliases`) y el checker
    // lo usa como *fallback* tras campo/método. Encadenado: `"x".saluda().grita()`.
    let files = &[
        ("texto", "pub fn saluda(n: string) -> string { \"hola \" + n }\npub fn grita(s: string) -> string { s.to_upper() }\n"),
        ("main", "from texto import saluda, grita;\nfn main() -> int { print(\"mundo\".saluda().grita()); 0 }\n"),
    ];
    for vm in [false, true] {
        let (out, code) = run_modules("ray_ufcs_import", "main", files, vm);
        assert_eq!(out.trim(), "HOLA MUNDO", "UFCS cross-module (vm={vm}): {out}");
        assert_eq!(code, 0);
    }
}

#[test]
fn ufcs_importado_con_alias() {
    // El alias de `from M import f as g` también vale para UFCS: `x.g(...)` → `M::f(x, ...)`.
    let files = &[
        ("lib", "pub fn doble(n: int) -> int { n * 2 }\n"),
        ("main", "from lib import doble as twice;\nfn main() -> int { print(21.twice()); 0 }\n"),
    ];
    for vm in [false, true] {
        let (out, code) = run_modules("ray_ufcs_alias", "main", files, vm);
        assert_eq!(out.trim(), "42", "UFCS con alias (vm={vm}): {out}");
        assert_eq!(code, 0);
    }
}

#[test]
fn ufcs_importado_no_rompe_acceso_a_campo() {
    // Seguridad: que exista una función importada homónima a un campo NO rompe el acceso al campo
    // (la resolución prueba campo antes que el alias importado).
    let files = &[
        ("lib", "pub fn name(c: int) -> string { \"FN\" }\n"),
        ("main", "from lib import name;\nstruct Caja { name: string }\nfn main() -> int { let c = Caja { name: \"CAMPO\" }; print(c.name); 0 }\n"),
    ];
    for vm in [false, true] {
        let (out, code) = run_modules("ray_ufcs_campo", "main", files, vm);
        assert_eq!(out.trim(), "CAMPO", "campo gana sobre función importada (vm={vm}): {out}");
        assert_eq!(code, 0);
    }
}
