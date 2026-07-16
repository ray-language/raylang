    use super::transpile;

    fn transpile_src(src: &str) -> String {
        let tokens = crate::lexer::lex(src).expect("lex");
        let mut prog = crate::parser::parse(tokens).expect("parse");
        crate::checker::check(&mut prog).expect("check");
        transpile(&prog).expect("transpile").source
    }

    #[test]
    fn transpila_fib_recursivo() {
        let rust = transpile_src(
            "fn fib(n: int) -> int { if (n < 2) { n } else { fib(n - 1) + fib(n - 2) } }\n\
             fn main() { print(fib(10)); }",
        );
        assert!(rust.contains("fn fib(mut n: i64) -> i64"), "{}", rust);
        assert!(rust.contains("fib((n - 1i64))"), "{}", rust);
        assert!(rust.contains("fib(10i64).ray_show()"), "{}", rust);
    }

    #[test]
    fn transpila_bucle_for_rango() {
        let rust = transpile_src(
            "fn main() { var acc: int = 0; for i in 0..100 { acc = acc + i; } print(acc); }",
        );
        assert!(rust.contains("for i in 0i64..100i64"), "{}", rust);
        assert!(rust.contains("let mut acc: i64 = 0i64"), "{}", rust); // anotación emitida (pina inferencia)
    }

    #[test]
    fn transpila_strings_concat_y_clon() {
        let rust = transpile_src(
            "fn greet(name: string) -> string { \"hi \" + name }\n\
             fn main() -> int { let g = greet(\"bob\"); print(g); g.len() }",
        );
        // string → Rc<str>; concat via format!; el `g` heap se clona al leer.
        assert!(rust.contains("fn greet(mut name: Rc<str>) -> Rc<str>"), "{}", rust);
        assert!(rust.contains("Rc::<str>::from(format!"), "{}", rust);
        assert!(rust.contains("g.clone()"), "{}", rust);
    }

    #[test]
    fn transpila_arreglos_split_join() {
        let rust = transpile_src(
            "fn main() -> int { var xs: [string] = []; xs.push(\"a\"); \
             let parts = \"a,b,c\".split(\",\"); parts.join(\"-\").len() }",
        );
        assert!(rust.contains("Rc<std::cell::RefCell<Vec<Rc<str>>>>"), "{}", rust);
        assert!(rust.contains(".borrow_mut().push("), "{}", rust);
        assert!(rust.contains("__ray_split(&"), "{}", rust);
        assert!(rust.contains("__ray_join(&"), "{}", rust);
    }

    #[test]
    fn transpila_map_add_to_get() {
        let rust = transpile_src(
            "fn main() -> int { var m: Map<string, int> = Map.new(); m.add_to(\"a\", 1); \
             m.add_to(\"a\", 2); m.get(\"a\").unwrap_or(0) }",
        );
        assert!(rust.contains("HashMap"), "{}", rust);
        assert!(rust.contains(".entry("), "{}", rust);
        assert!(rust.contains(".or_insert(0i64) += "), "{}", rust);
        assert!(rust.contains(".get(&") && rust.contains(".unwrap_or("), "{}", rust);
    }

    #[test]
    fn transpila_struct_y_enum_match() {
        let rust = transpile_src(
            "struct P { x: int, y: int }\n\
             enum Shape { Circle(float), Dot }\n\
             fn area(s: Shape) -> float { match (s) { Shape.Circle(r) => r * r, Shape.Dot => 0.0 } }\n\
             fn main() -> int { let p = P { x: 1, y: 2 }; print(area(Shape.Circle(2.0))); p.x }",
        );
        assert!(rust.contains("struct P {"), "{}", rust);
        assert!(rust.contains("enum Shape {"), "{}", rust);
        assert!(rust.contains("Rc::new(std::cell::RefCell::new(P {"), "{}", rust);
        assert!(rust.contains("Rc::new(Shape::Circle(2.0f64))"), "{}", rust);
        assert!(rust.contains("match &*") && rust.contains("Shape::Circle(r)"), "{}", rust);
        assert!(rust.contains("RayShow for Rc<std::cell::RefCell<P>>"), "{}", rust);
    }

    #[test]
    fn transpila_option_match_y_try() {
        let rust = transpile_src(
            "fn f(s: string) -> Option<int> { let n = parse_int(s)?; Option.Some(n + 1) }\n\
             fn main() -> int { match (f(\"7\")) { Option.Some(v) => v, Option.None => 0 } }",
        );
        // Option → nativo de Rust: firma Option<i64>, Some(...), `?`, y match con Some/None (sin Rc).
        assert!(rust.contains("-> Option<i64>"), "{}", rust);
        assert!(rust.contains(".parse::<i64>().ok())?"), "{}", rust);
        assert!(rust.contains("Some(") && rust.contains("None"), "{}", rust);
        assert!(rust.contains("match &") && !rust.contains("Option::"), "{}", rust);
    }

    #[test]
    fn transpila_closures_y_map() {
        let rust = transpile_src(
            "fn apply(f: fn(int) -> int, x: int) -> int { f(x) }\n\
             fn main() -> int { \
               let sq: fn(int) -> int = fn(n: int) -> int { n * n }; \
               let xs = [1, 2, 3]; let ys = xs.map(fn(x: int) -> int { x + 1 }); \
               apply(sq, 4) + ys.len() }",
        );
        assert!(rust.contains("Rc<dyn Fn(i64) -> i64>"), "{}", rust); // función-valor
        assert!(rust.contains("Rc::new(move |n: i64| -> i64"), "{}", rust); // anónima → closure move
        assert!(rust.contains(".iter().map(|__x| __f(__x.clone()))"), "{}", rust); // map → iterador
    }

    #[test]
    fn transpila_funciones_genericas() {
        let rust = transpile_src(
            "fn id<T>(x: T) -> T { x }\n\
             fn apply<T, U>(f: fn(T) -> U, x: T) -> U { f(x) }\n\
             fn neg(b: bool) -> bool { !b }\n\
             fn main() -> int { let a: int = id(5); print(apply(neg, false)); a }",
        );
        assert!(rust.contains("fn id<T: Clone + RayShow + 'static>(mut x: T) -> T"), "{}", rust);
        assert!(rust.contains("fn apply<T:") && rust.contains("U:"), "{}", rust);
        assert!(rust.contains("apply(Rc::new(neg)"), "{}", rust); // función como valor → Rc::new(fn)
    }

    #[test]
    fn transpila_tipos_genericos() {
        let rust = transpile_src(
            "struct Par<A, B> { a: A, b: B }\n\
             enum Caja<T> { Llena(T), Vacia }\n\
             fn saca(c: Caja<int>) -> int { match (c) { Caja.Llena(v) => v, Caja.Vacia => 0 } }\n\
             fn main() -> int { let p = Par { a: 1, b: true }; saca(Caja.Llena(9)) }",
        );
        assert!(rust.contains("struct Par<A: Clone"), "{}", rust);
        assert!(rust.contains("enum Caja<T: Clone"), "{}", rust);
        assert!(rust.contains("Caja::Llena(v)"), "{}", rust); // match de enum genérico
        assert!(rust.contains("Rc::new(Caja::Llena(9i64))"), "{}", rust);
    }

    #[test]
    fn transpila_traits_despacho_estatico() {
        let rust = transpile_src(
            "trait Valor { fn valor(self) -> int; }\n\
             struct P { x: int }\n\
             impl Valor for P { fn valor(self) -> int { self.x } }\n\
             fn main() -> int { let p = P { x: 7 }; p.valor() }",
        );
        // El método de trait se baja a una función manglada `P#valor` → `P_HH_valor` (erasure, M9).
        assert!(rust.contains("fn P_HH_valor(mut __self: Rc<std::cell::RefCell<P>>) -> i64"), "{}", rust);
        assert!(rust.contains("P_HH_valor("), "{}", rust);
        // Trait propio RayShow para el `Show` de raylang (Display no sirve con Rc<RefCell>).
        assert!(rust.contains("trait RayShow"), "{}", rust);
    }

    #[test]
    fn transpila_tuplas() {
        let rust = transpile_src(
            "fn divmod(a: int, b: int) -> (int, int) { (a / b, a % b) }\n\
             fn main() -> int { let d = divmod(17, 5); let (q, r) = divmod(23, 4); d.0 + q + r }",
        );
        assert!(rust.contains("-> (i64, i64,)"), "{}", rust); // tupla nativa de Rust
        assert!(rust.contains("let (q, r) = "), "{}", rust); // desestructuración
        assert!(rust.contains(".0"), "{}", rust); // acceso por índice de tupla
    }

    #[test]
    fn transpila_trait_objects_dyn() {
        let rust = transpile_src(
            "trait Figura { fn area(self) -> int; }\n\
             struct Cuad { lado: int }\n\
             impl Figura for Cuad { fn area(self) -> int { self.lado * self.lado } }\n\
             fn total(f: dyn Figura) -> int { f.area() }\n\
             fn main() -> int { total(Cuad { lado: 3 }) }",
        );
        // dyn → struct de closures que capturan el concreto (sin Box<dyn Any>, sin data).
        assert!(rust.contains("struct __dyn_Figura"), "{}", rust);
        assert!(rust.contains("area: Rc<dyn Fn() -> i64>"), "{}", rust);
        assert!(rust.contains("let __c = "), "{}", rust); // captura del concreto en la coerción
        assert!(rust.contains(".borrow().area.clone())"), "{}", rust); // despacho dinámico
    }

    #[test]
    fn transpila_bytes() {
        // bytes → Rc<[u8]>; literal b"..."; to_bytes/sub_bytes/from_utf8; render en hex (como la VM).
        let rust = transpile_src(
            "fn main() -> int { let b = \"Hi\".to_bytes(); let l = b\"xy\"; \
             print(to_string(b)); print(to_string(l)); print(to_string(b.sub_bytes(0, 1))); b.len() }",
        );
        assert!(rust.contains("Rc<[u8]>"), "tipo bytes: {}", rust);
        assert!(rust.contains(".as_bytes()"), "to_bytes → as_bytes: {}", rust);
        assert!(rust.contains("Rc::<[u8]>::from(vec!["), "literal de bytes: {}", rust);
        assert!(rust.contains("impl RayShow for Rc<[u8]>"), "render hex: {}", rust);
        assert!(rust.contains("{:02x}"), "hex minúsculas: {}", rust);
    }

    #[test]
    fn var_capturada_y_mutada_por_closure_va_en_una_celda() {
        // B1: una `var` capturada+mutada por una closure (patrón contador) vive en `Rc<RefCell<T>>`; se
        // lee con `.borrow()`, se escribe con `.borrow_mut()`, y la closure captura un clon del Rc.
        let rust = transpile_src(
            "fn contador() -> fn() -> int { var n: int = 0; fn() -> int { n = n + 1; n } }\n\
             fn main() { let c = contador(); print(c()); }",
        );
        assert!(rust.contains("let n = Rc::new(std::cell::RefCell::new("), "n es una celda: {}", rust);
        assert!(rust.contains("*n.borrow_mut() ="), "escritura por borrow_mut: {}", rust);
        assert!(rust.contains("n.borrow().clone()"), "lectura por borrow: {}", rust);
        assert!(rust.contains("let n = n.clone();"), "pre-clon al capturar: {}", rust);
    }

    #[test]
    fn var_mutable_no_capturada_sigue_siendo_local_normal() {
        // Una `var` mutada pero NO capturada por ninguna closure NO va en celda (sin coste): `let mut`.
        let rust = transpile_src("fn main() { var x: int = 0; x = x + 1; print(x); }");
        assert!(rust.contains("let mut x: i64 = 0i64;"), "x es local normal: {}", rust);
        assert!(!rust.contains("let x = Rc::new(std::cell::RefCell"), "x NO va en celda: {}", rust);
    }

    #[test]
    fn ffi_emite_extern_c_y_wrapper_con_marshalling() {
        // FFI (M41): `extern "m" { fn sqrt(x: float) -> float; }` → una decl `extern "C"` del símbolo C
        // (`__ffi_sqrt` con `#[link_name]`, bajo `#[link(name = "m")]`) + un wrapper que llama en `unsafe`.
        let rust = transpile_src(
            "extern \"m\" { fn sqrt(x: float) -> float; }\n\
             fn main() { print(sqrt(2.0)); }",
        );
        assert!(rust.contains("#[link(name = \"m\")]"), "link a libm: {}", rust);
        assert!(rust.contains("#[link_name = \"sqrt\"]"), "link_name del símbolo: {}", rust);
        assert!(rust.contains("fn __ffi_sqrt(__a0: f64) -> f64"), "decl extern C: {}", rust);
        assert!(rust.contains("unsafe { __ffi_sqrt("), "wrapper llama en unsafe: {}", rust);
    }

    #[test]
    fn ffi_retorno_int_es_c_int_de_32_bits() {
        // El retorno `int` de una extern es C `int` (32 bits, sign-extendido), NO `long`: declararlo i64
        // rompería el ABI (el EOF -1 de `fgetc` se vería positivo). Debe ir por `c_int` + `as i64`.
        let rust = transpile_src(
            "extern \"c\" { fn fgetc(s: ptr) -> int; }\n\
             fn main() { print(0); }",
        );
        assert!(rust.contains("-> std::os::raw::c_int;"), "retorno c_int: {}", rust);
        assert!(rust.contains("__r as i64"), "sign-extiende a i64: {}", rust);
        // libc no lleva #[link] (ya enlazada).
        assert!(!rust.contains("#[link(name = \"c\")]"), "libc implícita: {}", rust);
    }

    #[test]
    fn for_sobre_iterador_de_usuario_baja_a_un_loop_con_next() {
        // B2: `for x in it` sobre un `impl Iterator<T>` de usuario baja a un `loop` que llama `next(it)`
        // hasta `None`, ligando cada `Some(x)`. El iterador se liga a `__it` una vez (estado persistente).
        let rust = transpile_src(
            "struct R { a: int, b: int }\n\
             impl Iterator<int> for R { fn next(self) -> Option<int> { if (self.a >= self.b) { Option.None } else { let v: int = self.a; self.a = self.a + 1; Option.Some(v) } } }\n\
             fn r(a: int, b: int) -> R { R { a: a, b: b } }\n\
             fn main() { for n in r(1, 4) { print(n); } }",
        );
        assert!(rust.contains("let __it = "), "liga el iterador a __it: {}", rust);
        assert!(rust.contains("(__it.clone()) { Some("), "loop match next(__it): {}", rust);
        assert!(rust.contains("None => break"), "corta en None: {}", rust);
    }

    #[test]
    fn for_sobre_enumerate_destructura_la_tupla() {
        // `for (i, x) in it.enumerate()`: `next` da `Option<(int, T)>`; el `T` se resuelve unificando el
        // tipo del iterador con el `self` de `next` a TRAVÉS de la cadena de adaptadores (unify/subst_type
        // recurren en structs genéricos y tuplas). Baja a `match … { Some((i, x)) => …, None => break }`.
        let rust = transpile_src(
            "fn main() { for (i, x) in [10, 20].iter().enumerate() { print(i); print(x); } }",
        );
        assert!(rust.contains("{ Some((") && rust.contains(")) => "), "destructura la tupla: {}", rust);
    }

    #[test]
    fn campo_closure_se_llama_desenvolviendolo() {
        // Llamar un campo de tipo función (`self.step()`, como en `Iter#next`) → `(r.borrow().step.clone())()`.
        let rust = transpile_src(
            "struct Caja { f: fn() -> int }\n\
             fn tira(c: Caja) -> int { c.f() }\n\
             fn main() { print(tira(Caja { f: fn() -> int { 7 } })); }",
        );
        assert!(rust.contains(".borrow().f.clone())("), "llama el campo-closure: {}", rust);
    }

    #[test]
    fn una_funcion_no_transpilable_queda_como_stub_que_panica() {
        // Una función no-main cuyo cuerpo cae fuera del subconjunto (aquí un canal de un struct no-Send,
        // como el real de metrics_server) se emite como STUB que panica, con su firma → el programa
        // COMPILA; si el flujo real no la llama, corre igual que la VM. Antes se OMITÍA y una llamada
        // colgante hacía fallar rustc.
        let rust = transpile_src(
            "struct Msg { x: int }\n\
             fn arranca() -> int { let c: Channel<Msg> = Channel.new(); send(c, Msg { x: 1 }); 1 }\n\
             fn main() -> int { print(42); 0 }",
        );
        assert!(
            rust.contains("fn arranca() -> i64 { panic!("),
            "arranca es un stub que panica: {}",
            rust
        );
        // main y worker SÍ se transpilan normalmente.
        assert!(rust.contains("fn ray_main()"), "main normal: {}", rust);
    }

    #[test]
    fn concat_de_bytes_y_arreglos() {
        // `a + b` para bytes → concat de slices en un Rc<[u8]> nuevo; para arreglos → arreglo nuevo con
        // los elementos de ambos. Antes caían al `+` genérico, que Rust rechaza (Rc<[u8]>/Vec no tienen Add).
        let by = transpile_src(
            "fn main() { let a = \"x\".to_bytes(); let b = \"y\".to_bytes(); print(to_string(a + b)); }",
        );
        assert!(by.contains("Rc::<[u8]>::from([&*"), "concat de bytes: {}", by);
        let ar = transpile_src(
            "fn main() { let a = [1, 2]; let b = [3]; let c = a + b; print(c.len()); }",
        );
        assert!(ar.contains(".borrow().clone(); __v.extend("), "concat de arreglos: {}", ar);
    }

    #[test]
    fn len_de_string_cuenta_caracteres() {
        // `len(string)` = nº de CARACTERES (como la VM), no bytes → `.chars().count()` (clave con UTF-8).
        let rust = transpile_src("fn main() { let s = \"ab\"; print(s.len()); }");
        assert!(rust.contains(".chars().count() as i64"), "len de string por caracteres: {}", rust);
    }

    #[test]
    fn una_funcion_de_usuario_gana_a_un_builtin_del_prelude() {
        // Si el usuario define su propio `get_or` (aquí 2 args), NO se descarta como el builtin del prelude
        // (que está en LINE_BASE): se emite y su llamada baja a `get_or(...)` ordinario (override).
        let rust = transpile_src(
            "fn get_or(m: Map<string, int>, k: string) -> int { match (get(m, k)) { Option.Some(v) => v, Option.None => 0 } }\n\
             fn main() { var m: Map<string, int> = Map.new(); m.insert(\"a\", 1); print(get_or(m, \"a\")); }",
        );
        assert!(rust.contains("fn get_or("), "la fn get_or del usuario se emite: {}", rust);
    }

    #[test]
    fn campo_funcion_se_muestra_como_marcador() {
        // Un campo/payload de tipo función se renderiza `<fn>` (como el Display del runtime), no vía
        // `.ray_show()` (que no existe para `Rc<dyn Fn…>`).
        let rust = transpile_src(
            "struct Bx { f: fn(int) -> int, n: int }\n\
             fn main() { print(Bx { f: fn(x: int) -> int { x }, n: 1 }); }",
        );
        assert!(rust.contains("\"<fn>\""), "campo función como <fn>: {}", rust);
    }

    #[test]
    fn emite_rayshow_para_map() {
        // Un enum con una variante que lleva un Map (patrón `Json.JObject(Map<string, Json>)`): el
        // RayShow generado para el enum recurre al del Map. Debe existir el impl de Map (`Map{k: v}`,
        // pares ordenados) o rustc no compila el (posiblemente muerto) RayShow del enum.
        let rust = transpile_src(
            "enum J { JInt(int), JObj(Map<string, J>) }\n\
             fn main() { print(J.JInt(1)); }",
        );
        assert!(
            rust.contains("RayShow for Rc<std::cell::RefCell<std::collections::HashMap<K, V>>>"),
            "impl RayShow para Map: {}",
            rust
        );
        assert!(rust.contains("Map{{{}}}"), "formato Map{{…}}: {}", rust);
    }

    #[test]
    fn transpila_multi_modulo_mangla_los_tipos() {
        // Un proyecto multi-módulo: el loader namespaca los tipos a `modulo::Tipo`. El transpilador debe
        // manglarlos a un identificador Rust válido (`figuras_CC_Rect`), no dejar el `::`; en las cadenas
        // de Display de RayShow debe usar el nombre LOCAL (`Rect`), como la VM.
        let loaded = match crate::loader::load(std::path::Path::new("examples/modulos/main.ray")) {
            Ok(l) => l,
            Err(_) => panic!("no se pudo cargar modulos/main.ray"),
        };
        let mut prog = loaded.program;
        crate::checker::check(&mut prog).expect("check");
        let rust = transpile(&prog).expect("transpile").source;
        assert!(rust.contains("figuras_CC_Rect"), "tipo namespacado manglado: {}", rust);
        // No debe quedar `::` en un IDENTIFICADOR de tipo Rust (struct/enum def, referencia): esos van
        // manglados. (El `::` sí aparece en las cadenas de Display de RayShow, entre comillas.)
        assert!(!rust.contains("struct figuras::"), "def de struct manglada: {}", rust);
        assert!(!rust.contains("RefCell<figuras::"), "referencia de tipo manglada: {}", rust);
        // El render default de `print` (RayShow) usa el nombre COMPLETO namespacado, como la VM.
        assert!(rust.contains("\"figuras::Rect {{"), "Display con nombre completo: {}", rust);
    }

    #[test]
    fn transpila_std_math() {
        // `import std/math` necesita el loader; cargamos el ejemplo real y comprobamos el mapeo a los
        // métodos de `f64` de Rust (misma impl que la VM → mismo resultado; verificado byte a byte aparte).
        let loaded = match crate::loader::load(std::path::Path::new("examples/basics/matematicas.ray")) {
            Ok(l) => l,
            Err(_) => panic!("no se pudo cargar matematicas.ray"),
        };
        let mut prog = loaded.program;
        crate::checker::check(&mut prog).expect("check");
        let rust = transpile(&prog).expect("transpile").source;
        assert!(rust.contains(").sqrt()"), "{}", rust);
        assert!(rust.contains(").powf("), "{}", rust);
        assert!(rust.contains(").floor()"), "{}", rust);
        assert!(rust.contains(").abs()"), "{}", rust); // ad-hoc int|float
        assert!(rust.contains(").min("), "{}", rust);
        assert!(rust.contains("std::f64::consts::PI"), "{}", rust);
        assert!(rust.contains("std::f64::consts::E"), "{}", rust);
        // No debe emitir los wrappers del módulo (`fn ...sqrt`) ni el primitivo `__sqrt`.
        assert!(!rust.contains("__sqrt"), "{}", rust);
    }

    #[test]
    fn transpila_contains_y_parse_float() {
        // contains ad-hoc (string subcadena / arreglo pertenencia) + parse_float → Option<float>.
        let rust = transpile_src(
            "fn main() -> int { let ok = \"abc\".contains(\"b\"); let xs: [int] = [1, 2]; \
             let m = xs.contains(2); match (parse_float(\"1.5\")) { Option.Some(f) => { if (ok && m) { 1 } else { 0 } }, Option.None => 0 } }",
        );
        assert!(rust.contains(".contains(&*"), "string contains: {}", rust);
        assert!(rust.contains(".iter().any(|__e| *__e == __x)"), "array contains: {}", rust);
        assert!(rust.contains(".parse::<f64>().ok()"), "parse_float: {}", rust);
    }

    #[test]
    fn get_or_no_builtin_no_crashea() {
        // Un `get_or` con aridad distinta a 3 (no es el del prelude) NO debe hacer ICE (antes: pánico por
        // eff[2]); debe dar un error de transpilación limpio (cae al fallback).
        let tokens = crate::lexer::lex(
            "fn get_or(m: Map<string, int>, k: string) -> int { m.get_or(k, 0) }\n\
             fn main() { var m: Map<string, int> = Map.new(); m.insert(\"a\", 1); print(get_or(m, \"a\")); }",
        )
        .unwrap();
        let mut prog = crate::parser::parse(tokens).unwrap();
        // Puede ser válido o no en el checker; lo importante es que transpile() no PANIQUE.
        if crate::checker::check(&mut prog).is_ok() {
            let _ = super::transpile(&prog); // no debe hacer panic
        }
    }

    #[test]
    fn transpila_index_of() {
        // index_of(s, sub) -> Option<int>: índice por CARÁCTER de la subcadena (helper __ray_index_of).
        let rust = transpile_src(
            "fn main() -> int { match (index_of(\"hello\", \"ll\")) { Option.Some(i) => i, Option.None => 0 - 1 } }",
        );
        assert!(rust.contains("__ray_index_of(&*"), "index_of → helper: {}", rust);
        assert!(rust.contains("fn __ray_index_of(s: &str, sub: &str) -> Option<i64>"), "helper por carácter: {}", rust);
    }

    #[test]
    fn transpila_char_code() {
        // char_code(c) -> int (code point); char_from_code(n) -> Option<char>.
        let rust = transpile_src(
            "fn main() -> int { let n = char_code('A'); \
             match (char_from_code(n + 1)) { Option.Some(c) => char_code(c), Option.None => 0 } }",
        );
        assert!(rust.contains("as u32 as i64)"), "char_code → code point: {}", rust);
        assert!(rust.contains("char::from_u32("), "char_from_code → Option<char>: {}", rust);
    }

    #[test]
    fn push_que_lee_el_mismo_arreglo_no_doble_borra() {
        // `w.push(w[i] + w[j])` (típico en cripto): el valor debe evaluarse a un TEMP antes del borrow_mut,
        // si no el RefCell entra en doble borrow y PANICA en runtime. Regresión de sha256/hmac.
        let rust = transpile_src(
            "fn main() -> int { var w: [int] = [1, 2, 3]; w.push(w[0] + w[2]); w[3] }",
        );
        // el valor se saca a __v antes del borrow_mut().push.
        assert!(rust.contains("{ let __v = "), "push evalúa el valor a un temp: {}", rust);
        assert!(rust.contains(".borrow_mut().push(__v);"), "push del temp: {}", rust);
    }

    #[test]
    fn transpila_udp() {
        // Primitivos __udp_* (los llaman los wrappers de udp.ray): bind/send → [string]; recv → [bytes].
        let rust = transpile_src(
            "fn main() -> int { let b = __udp_bind(\"127.0.0.1\", 0); \
             let h = match (parse_int(b[1])) { Option.Some(x) => x, Option.None => 0 }; \
             let s = __udp_send_to(h, \"127.0.0.1\", 9999, \"x\".to_bytes()); \
             let r = __udp_recv_from(h); print(to_string(r.len())); 0 }",
        );
        assert!(rust.contains("Udp(std::net::UdpSocket)"), "handle Udp: {}", rust);
        assert!(rust.contains("__ray_udp_bind(&*") && rust.contains("__ray_udp_send_to("), "bind/send: {}", rust);
        assert!(rust.contains("__ray_udp_recv_from("), "recv: {}", rust);
        assert!(rust.contains(".recv_from(&mut buf)"), "recv bloqueante: {}", rust);
    }

    #[test]
    fn transpila_sockets_tcp() {
        // std::net::{tcp_connect,tcp_listen,tcp_accept,socket_read,socket_write,local_port} → std::net.
        let dir = std::env::temp_dir().join(format!("ray_net_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let src = "import std/net;\n\
             fn main() -> int {\n\
               let l = match (net.tcp_listen(\"127.0.0.1\", 0)) { Result.Ok(h) => h, Result.Err(e) => 0 - 1 };\n\
               let p = net.local_port(l);\n\
               let c = match (net.tcp_accept(l)) { Result.Ok(h) => h, Result.Err(e) => 0 - 1 };\n\
               let m = match (net.socket_read(c)) { Result.Ok(s) => s, Result.Err(e) => \"\" };\n\
               let _ = net.socket_write(c, m); p }\n";
        std::fs::write(dir.join("main.ray"), src).unwrap();
        let loaded = match crate::loader::load(&dir.join("main.ray")) {
            Ok(l) => l,
            Err(_) => panic!("no se pudo cargar el programa de sockets"),
        };
        let mut prog = loaded.program;
        crate::checker::check(&mut prog).expect("check");
        let rust = transpile(&prog).expect("transpile").source;
        assert!(rust.contains("Tcp(std::net::TcpStream)"), "handle Tcp: {}", rust);
        assert!(rust.contains("__ray_tcp_listen(") && rust.contains("__ray_tcp_accept("), "listen/accept: {}", rust);
        assert!(rust.contains("__ray_socket_read(") && rust.contains("__ray_socket_write("), "read/write: {}", rust);
        assert!(rust.contains("__ray_local_port("), "local_port: {}", rust);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn transpila_time_y_random() {
        // std::time::{now,monotonic,sleep} + std::random::{next,below,seed} (necesitan el loader por el
        // `import`). No deterministas → aquí solo se comprueba la ESTRUCTURA del Rust emitido.
        let dir = std::env::temp_dir().join(format!("ray_tr_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let src = "import std/time;\nimport std/random;\n\
             fn main() -> int { let t = time.now() + time.monotonic(); random.seed(1); \
             let r = random.next(); let n = random.below(6); if (r > 0.0) { n + t } else { 0 } }\n";
        std::fs::write(dir.join("main.ray"), src).unwrap();
        let loaded = match crate::loader::load(&dir.join("main.ray")) {
            Ok(l) => l,
            Err(_) => panic!("no se pudo cargar el programa de time/random"),
        };
        let mut prog = loaded.program;
        crate::checker::check(&mut prog).expect("check");
        let rust = transpile(&prog).expect("transpile").source;
        assert!(rust.contains("SystemTime::now()"), "now → SystemTime: {}", rust);
        assert!(rust.contains("__ray_monotonic()"), "monotonic: {}", rust);
        assert!(rust.contains("__ray_random_f64()") && rust.contains("__ray_random_int("), "random: {}", rust);
        assert!(rust.contains("fn __ray_next_u64()"), "PRNG SplitMix64 emitido: {}", rust);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn transpila_indexar_bytes() {
        // b[i] sobre bytes → el octeto como int (Rc<[u8]>, sin borrow; envuelto en () por el `as i64`).
        let rust = transpile_src(
            "fn main() -> int { let b = \"Hi\".to_bytes(); print(to_string(b[0])); b[1] }",
        );
        assert!(rust.contains("[0i64 as usize] as i64)"), "b[i] → octeto int con paréntesis: {}", rust);
        assert!(!rust.contains("b.clone().borrow()"), "bytes no lleva borrow: {}", rust);
    }

    #[test]
    fn transpila_mas_builtins_string_y_eprint() {
        // eprint (stderr), bytes_of, y el resto de builtins de string (→ métodos de str de Rust).
        let rust = transpile_src(
            "fn main() -> int { eprint(\"e\"); let b = bytes_of([1, 2]); \
             print(\"  x  \".trim().to_upper().to_lower()); \
             print(\"ab\".repeat(2).replace(\"a\", \"z\").substring(0, 3)); \
             if (\"hi\".starts_with(\"h\")) { b.len() } else { 0 } }",
        );
        assert!(rust.contains("eprintln!("), "eprint → eprintln!: {}", rust);
        assert!(rust.contains(".trim()") && rust.contains(".to_uppercase()"), "trim/to_upper: {}", rust);
        assert!(rust.contains(".repeat(") && rust.contains(".replace(&*"), "repeat/replace: {}", rust);
        assert!(rust.contains(".starts_with(&*"), "starts_with: {}", rust);
        assert!(rust.contains("*__x as u8"), "bytes_of: {}", rust);
    }

    #[test]
    fn transpila_canal_de_string() {
        // Channel<string>: el elemento viaja como repr SEND (Arc<str>), convertido al borde (send/recv).
        let rust = transpile_src(
            "fn main() -> int { let ch: Channel<string> = Channel.new(); \
             spawn(fn() { send(ch, \"hola\"); close(ch); }); \
             match (recv(ch)) { Option.Some(s) => s.len(), Option.None => 0 } }",
        );
        assert!(rust.contains("__RayChan<std::sync::Arc<str>>"), "canal de Arc<str>: {}", rust);
        assert!(rust.contains("std::sync::Arc::<str>::from(&*"), "send convierte a Arc<str>: {}", rust);
        assert!(rust.contains(".map(|__x| Rc::<str>::from(&*__x))"), "recv convierte de vuelta a Rc<str>: {}", rust);
    }

    #[test]
    fn rechaza_canal_de_struct() {
        // Un canal de struct (mutable, no-Send) → error claro (diferido).
        let tokens = crate::lexer::lex(
            "struct P { x: int } fn main() { let ch: Channel<P> = Channel.new(); send(ch, P { x: 1 }); }",
        )
        .unwrap();
        let mut prog = crate::parser::parse(tokens).unwrap();
        crate::checker::check(&mut prog).unwrap();
        assert!(super::transpile(&prog).is_err());
    }

    #[test]
    fn transpila_signals() {
        // signals() -> Channel<int> (M88.1): canal de señales del SO (self-pipe + FFI a libc).
        let rust = transpile_src(
            "fn main() -> int { let sig: Channel<int> = signals(); \
             match (recv(sig)) { Option.Some(n) => n, Option.None => 0 } }",
        );
        assert!(rust.contains("__ray_signals()"), "signals → __ray_signals: {}", rust);
        assert!(rust.contains("fn __ray_signals()"), "runtime de señales emitido: {}", rust);
        assert!(rust.contains("__ray_on_signal"), "handler de señal: {}", rust);
        assert!(rust.contains("signal(15,") && rust.contains("signal(2,"), "instala SIGTERM/SIGINT: {}", rust);
    }

    #[test]
    fn transpila_select() {
        // select([chs]) -> int (M12.4): índice del primer canal listo (poll del índice menor).
        let rust = transpile_src(
            "fn main() -> int { let a: Channel<int> = Channel.new(); let b: Channel<int> = Channel.new(); \
             let chs: [Channel<int>] = [a, b]; select(chs) }",
        );
        assert!(rust.contains("__ray_select(&"), "select → __ray_select: {}", rust);
        assert!(rust.contains("fn __ray_select<T>"), "runtime de select emitido: {}", rust);
    }

    #[test]
    fn transpila_structured_concurrency() {
        // Task/join/scope (M12.3): spawn → __ray_spawn (devuelve Task); join(t) → t.join(); scope → __ray_scope.
        let rust = transpile_src(
            "fn sq(n: int) -> int { n * n }\n\
             fn main() -> int { scope(fn() -> int { \
             let a: Task<int> = spawn(fn() -> int { sq(3) }); \
             let b: Task<int> = spawn(fn() -> int { sq(4) }); \
             join(a) + join(b) }) }",
        );
        assert!(rust.contains("__RayTask<i64>"), "Task → __RayTask: {}", rust);
        assert!(rust.contains("__ray_spawn(move ||"), "spawn → __ray_spawn: {}", rust);
        assert!(rust.contains("__ray_scope(move ||"), "scope → __ray_scope: {}", rust);
        assert!(rust.contains(".join()"), "join(t) → .join(): {}", rust);
    }

    #[test]
    fn transpila_concurrencia_csp() {
        // spawn + canales (M12.1/M12.2): Channel.new/bounded → __RayChan; send/recv/close; spawn → thread.
        let rust = transpile_src(
            "fn prod(c: Channel<int>) { send(c, 1); close(c); }\n\
             fn main() -> int { let c: Channel<int> = Channel.bounded(2); \
             spawn(fn() { prod(c); }); \
             match (recv(c)) { Option.Some(v) => v, Option.None => 0 } }",
        );
        assert!(rust.contains("__RayChan<i64>"), "canal → __RayChan: {}", rust);
        assert!(rust.contains("__RayChan::make(Some("), "bounded: {}", rust);
        assert!(rust.contains("__ray_spawn(move ||"), "spawn → hilo (vía __ray_spawn): {}", rust);
        assert!(rust.contains(".send("), "send: {}", rust);
        assert!(rust.contains(".recv()"), "recv: {}", rust);
        assert!(rust.contains(".close()"), "close de canal: {}", rust);
        // el canal capturado se clona antes del spawn (Arc bump → el original sigue usable).
        assert!(rust.contains("let c = c.clone();"), "clona el canal antes del spawn: {}", rust);
    }

    #[test]
    fn transpila_env() {
        // env(name) -> Option<string>: variable de entorno vía std::env::var(...).ok().
        let rust = transpile_src(
            "fn main() -> int { match (env(\"HOME\")) { Option.Some(v) => v.len(), Option.None => 0 } }",
        );
        assert!(rust.contains("std::env::var("), "env → std::env::var: {}", rust);
        assert!(rust.contains(".ok().map(Rc::<str>::from)"), "→ Option<string>: {}", rust);
    }

    #[test]
    fn transpila_args() {
        // args() → arreglo de string (argv tras el binario); a[i] indexa, a.len() cuenta.
        let rust = transpile_src(
            "fn main() -> int { let a = args(); \
             if (a.len() > 0) { a[0].len() } else { 0 } }",
        );
        assert!(rust.contains("std::env::args().skip(1)"), "{}", rust);
        assert!(rust.contains("Rc::<str>::from(__a)"), "{}", rust);
        // el arreglo se indexa/mide como cualquier `[string]` (borrow).
        assert!(rust.contains(".borrow().len() as i64"), "{}", rust);
    }

    #[test]
    fn transpila_operator_overloading_y_show_custom() {
        // `a + b` con `impl Add for Vec2` → llamada al método (`Vec2#add`); un `impl Show` CUSTOM se
        // respeta en `.show()` (llama a `Vec2#show`), mientras `print(x)` usaría el render default (RayShow).
        let rust = transpile_src(
            "struct Vec2 { x: int, y: int }\n\
             impl Add for Vec2 { fn add(self, o: Vec2) -> Vec2 { Vec2 { x: self.x + o.x, y: self.y + o.y } } }\n\
             impl Show for Vec2 { fn show(self) -> string { \"(${self.x}, ${self.y})\" } }\n\
             fn main() { let a = Vec2 { x: 1, y: 2 }; let b = Vec2 { x: 3, y: 4 }; print((a + b).show()); }",
        );
        assert!(rust.contains("Vec2_HH_add"), "operator+ → método: {}", rust); // suma vía impl Add
        assert!(rust.contains("Vec2_HH_show"), "impl Show custom emitido y llamado: {}", rust);
        // `.show()` NO debe mapearse a `.ray_show()` (eso daría el render default `Vec2 { x, y }`).
        assert!(rust.contains("fn Vec2_HH_show"), "el impl Show se emite: {}", rust);
    }

    #[test]
    fn transpila_enteros_con_tamano() {
        // u8/u32/u64 → nativos de Rust; literal tipado por contexto (200u8, elementos de [u8]); aritmética
        // envolvente (wrapping_*) entre valores sized para no chocar con el deny de overflow constante de
        // Rust; cast `as uN`/`as int`. Aritmética con vars sized (no literales) → dispara wrapping.
        let rust = transpile_src(
            "fn fnv(data: [u8]) -> u32 { var h: u32 = 2166136261; let p: u32 = 16777619; \
             for b in data { h = (h ^ b as u32) * p; } h }\n\
             fn main() -> int { let a: u8 = 200; let b: u8 = 100; let d: [u8] = [104, 105]; \
             (a + b) as int + fnv(d) as int }",
        );
        // El checker coacciona los literales con un Cast explícito → `(200i64 as u8)`; mi Cast lo emite.
        assert!(rust.contains("let a: u8 = (200i64 as u8)"), "u8 anotado + literal coaccionado: {}", rust);
        assert!(rust.contains("(104i64 as u8)"), "elemento de [u8] coaccionado: {}", rust);
        assert!(rust.contains(".wrapping_add("), "suma envolvente u8: {}", rust);
        assert!(rust.contains(".wrapping_mul("), "mult envolvente u32: {}", rust);
        assert!(rust.contains(" as u32)"), "cast a u32: {}", rust);
        assert!(rust.contains("as i64"), "cast a int: {}", rust);
    }

    #[test]
    fn transpila_from_conversion() {
        // `?` con From-conversion: el checker baja a un match con temps `$to`/`$te` y una llamada a la
        // conversión `AppError#from#string`. Verificamos que los `$` se manglan y la conversión se emite.
        let rust = transpile_src(
            "enum AppError { Lectura(string), Vacio }\n\
             impl From<string> for AppError { fn convert(o: string) -> AppError { AppError.Lectura(o) } }\n\
             fn leer(ok: bool) -> Result<int, string> { if (ok) { Result.Ok(1) } else { Result.Err(\"x\") } }\n\
             fn cargar(ok: bool) -> Result<int, AppError> { let v = leer(ok)?; Result.Ok(v) }\n\
             fn main() -> int { match (cargar(true)) { Result.Ok(v) => v, Result.Err(_) => 0 } }",
        );
        assert!(rust.contains("AppError_HH_from_HH_string"), "{}", rust); // la conversión se emite
        assert!(!rust.contains('$'), "los temps $ deben manglarse: {}", rust);
    }

    #[test]
    fn transpila_derive_eq() {
        // @derive(Eq) + bound T: Eq → paso de diccionarios (como los traits de usuario): el impl derivado
        // `Tipo#eq` se emite como función ordinaria, y `x.eq(y)` con x: T acotado llama al dict param.
        let rust = transpile_src(
            "@derive(Eq)\nstruct Punto { x: int, y: int }\n\
             fn iguales<T: Eq>(a: T, b: T) -> bool { a.eq(b) }\n\
             fn main() -> int { if (iguales(Punto { x: 1, y: 2 }, Punto { x: 1, y: 2 })) { 1 } else { 0 } }",
        );
        // el impl derivado se emite manglado (Punto#eq → Punto_HH_eq), no se salta.
        assert!(rust.contains("Punto_HH_eq"), "{}", rust);
        // la función acotada conserva el param-diccionario y NO se mapea a `==`.
        assert!(rust.contains("T_HH_Eq_HH_eq"), "{}", rust);
        assert!(!rust.contains("a.clone() == b.clone()"), "no debe mapear .eq() a ==: {}", rust);
    }

    #[test]
    fn transpila_for_sobre_map() {
        // for (k, v) in map → pares ordenados por clave (helper __ray_pairs), determinista como la VM.
        let rust = transpile_src(
            "fn main() { var m: Map<string, int> = Map.new(); m.insert(\"a\", 1); \
             for (k, v) in m { print(k + \": \" + to_string(v)); } }",
        );
        assert!(rust.contains("fn __ray_pairs<"), "{}", rust);
        assert!(rust.contains("for (k, v) in __ray_pairs(&"), "{}", rust);
    }

    #[test]
    fn rechaza_fuera_del_subconjunto() {
        // un `main` con `try_join` (une una Task devolviendo Result; aún fuera) → no transpilable.
        let tokens = crate::lexer::lex(
            "fn main() { let t: Task<int> = spawn(fn() -> int { 1 }); match (try_join(t)) { Result.Ok(v) => print(v), Result.Err(e) => print(0) } }",
        )
        .unwrap();
        let mut prog = crate::parser::parse(tokens).unwrap();
        crate::checker::check(&mut prog).unwrap();
        assert!(super::transpile(&prog).is_err());
    }

    #[test]
    fn transpila_io_de_entrada() {
        // input()/read_int() (prelude) → stdin; read_file/write_file/exists (std/fs, cualificados) → std::fs.
        let rust = transpile_src(
            "fn main() -> int { \
             match (input()) { Option.Some(l) => print(l), Option.None => print(\"eof\") } \
             match (read_int()) { Option.Some(n) => n, Option.None => 0 } }",
        );
        assert!(rust.contains("std::io::stdin().read_line("), "input/read_int leen stdin: {}", rust);
        assert!(rust.contains("trim_end_matches"), "quita el salto de línea como la VM: {}", rust);
    }
