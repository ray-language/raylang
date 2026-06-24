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
        ("a", "@derive(Show)\nstruct Node { v: int }\nenum Estado { On, Off }\npub fn f() -> int {\n  let n = Node { v: 7 };\n  let e = Estado.On;\n  print(n.mostrar());\n  match (e) { Estado.On => n.v + 100, Estado.Off => 0, }\n}\n"),
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
