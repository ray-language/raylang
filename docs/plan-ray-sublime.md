# Plan de mejoras impulsadas por `ray-sublime` (sep 2026)

`ray-apps/ray-sublime` es un editor de código escrito en raylang que hereda el ecosistema
declarativo de Sublime Text 4 (`.sublime-syntax`, esquemas de color, temas, keymaps): motor de
expresiones regulares propio con backtracking, lectores de YAML/plist/ZIP/JSON relajado, motor de
sintaxis con pila de contextos, `std/ui` + `web`. Su `IDEAS.md` recoge 33 entradas por etapa
(bugs 🐛, huecos de API 🕳️, documentación 📝 y mediciones 📊). Este documento las verifica contra
la toolchain actual y las convierte en trabajo ordenado.

**Verificación previa (8 sep 2026, `raylang 1.10.0`, main `d736be2`):** reproducidas con
ficheros de una línea las entradas 1, 2, 3, 5, 7, 10, 12, 13, 17, 18, 22, 24 y 27; confirmadas
por lectura del código las 8, 9, 14, 20 y 26. Hallazgo nuevo al reproducir 1 y 2: `compile`
ya devuelve `Ok`, el ICE salta en la **primera búsqueda**, y como el pánico ocurre en un hilo
del scheduler el proceso **se queda colgado** en vez de morir. La 31 (`} else` + espacios +
`if`) **no se reproduce** con 1.10.0 (el formateador de M189 mantiene `else if` y los
comentarios en su sitio); el colapso de llamadas multilínea que caben en 100 columnas es la
forma canónica, no un bug. La 33 ya tiene respuesta: `std/collections/set` existe.

## 1. Criterio de orden

1. **Bugs con ICE, cuelgue o paridad VM/nativo rota** antes que nada: son los que hacen perder
   ramas enteras (la 27 costó una a medio construir).
2. **Huecos pequeños de stdlib y checker** que la app esquiva con código propio y que cualquier
   programa que procese texto va a repetir (indexación de strings, `sort_by`, `Option.None`
   como argumento, `fs`).
3. **Herramientas y documentación**: baratas, transversales, y la mayoría son mensajes o alias.
4. **Formatos en `std`**: útiles pero grandes; se ordenan por relación coste/uso.
5. **Decisiones de lenguaje** (`return` como expresión, copia de structs, intérprete
   embebible, trazas): las toma el usuario con la SPEC delante.

Las entradas 15, 16, 19, 25, 28, 30 y 32 son mediciones o lecciones de la propia app: se
recogen en §7 como datos, sin trabajo asociado salvo lo que ya cae en otro lote.

## 2. Lote G — bugs (primero)

### G1. `std/regex`: rechazar lo que no soporta, y que un ICE nunca cuelgue (entradas 1, 2, 3) — M ✅ M211

Hoy `compile("(?=foo)")` y `compile("\\p{Lu}+")` devuelven `Ok` y la primera `search` provoca
un pánico en el traductor al dialecto del crate; `(a)\\1` y `\\Gab` compilan y devuelven `false`
en silencio. Dos piezas:

- El **validador** de `std/regex` (`examples/stdlib/regex.ray`, el parser propio que corre en los
  tres motores) rechaza con `Err` claro todo lo que el motor no implementa: look-around
  (`(?=` `(?!` `(?<=` `(?<!`), backreferences (`\\1`…`\\9`, `\\k<n>`), `\\G`, cuantificadores
  posesivos y grupos atómicos, `\\p{…}`/`\\P{…}`, `\\h`, `&&` en clases. Mensaje:
  `"regex: look-around is not supported"`, etc. La REFERENCE lista explícitamente lo excluido.
- Con el validador delante, la traducción del runtime solo recibe el subconjunto documentado y
  su `unwrap` vuelve a ser un ICE de verdad (un bug nuestro), no una vía que el usuario pueda
  disparar. Y un pánico en un hilo del scheduler no puede dejar el proceso esperando para
  siempre: el hook lo presenta como ICE y termina el proceso con 101, venga del hilo que venga.

Hecho (M211): 18 construcciones rechazadas por nombre, idéntico en VM, intérprete y nativo; un
pánico inyectado en un worker (`RAYLANG_DEBUG_PANIC_WORKER`) termina con 101 en menos de un
segundo, no cuelga.

### G2. `impl Ord` de usuario + `sort` rompe el binario nativo (entrada 27) — M ✅ M212

`sort(rs)` con `impl Ord for Range` corre en la VM (`1 5`) y en nativo falla con
`E0277: the trait bound Range: Ord is not satisfied`: `__ray_sort` exige el `Ord` de Rust y el
transpilador no lo emite para tipos de usuario. Misma familia que IDEAS §63 (`[float]`). Opción
elegida: cuando el elemento es un tipo de usuario con `impl Ord`, emitir la ordenación con el
`less` del usuario (`sort_by(|a, b| …)` sobre `Tipo#less`), estable como en la VM; los
primitivos siguen por `__ray_sort`. Test en `native_corpus`/`native_differential`: el programa de
10 líneas de la entrada, byte-idéntico. Es la pieza que hace viable también G4 (`sort_by`).

## 3. Lote H — checker y stdlib pequeña

### H1. Indexar un string por carácter deja de ser cuadrático (entrada 12) — L (decisión)

**Diagnóstico (8 sep):** el coste no es solo el escaneo hasta `i`: `HeapValue::Str` es un `String`
propio, así que cada `s[i]` **clona la cadena entera** al cargar la variable y además recorre la
cadena para decidir si es ASCII — dos O(n) por acceso. Una caché de posición no lo arregla. La
corrección real es representar los strings de la VM como `Rc<str>` compartidos (~230 sitios en
`vm/mod.rs`, más `gc`/`transfer`): una sesión, y de paso desaparece la copia en cada paso de string
por toda la VM. Va a decisión del usuario como arco propio (M213).

Medido hoy: 40 000 caracteres, `s[i]` en bucle 370 ms frente a 36 ms con `chars()`. Fix (c) de
la entrada: el runtime cachea la última posición (byte, carácter) por string y el acceso
secuencial hacia delante o atrás se vuelve amortizado O(1); el acceso aleatorio sigue O(n) y la
referencia lo dice junto a `s[i]`, con la regla práctica de la entrada 23 (`chars()` una vez para
texto; `bytes` se indexa directo). Aplica a VM, intérprete y nativo (mismo `Value`).

### H2. `Option.None` como argumento de una función genérica infiere del parámetro (entrada 17) — M ✅ M214

`out.push(Option.None)` con `out: [Option<string>]` pide anotación aunque el tipo está a la
vista. La inferencia de M204 (brazos de `match`) tiene el mismo espíritu: el parámetro
`T` de `push` ya está fijado por el receptor (`[Option<string>]`) antes de chequear el
argumento; basta pasarlo como tipo esperado. Espejo selfhost. Test: `push`, `insert` de `Map` y
una función de usuario `fn f(x: Option<int>)` con `f(Option.None)`.

### H3. `sort_by(xs, cmp)` y `sort_by_key(xs, key)` (entrada 18) — S–M

En el prelude (merge estable con el comparador, como `sort`), con opcode en la VM y helper
nativo. Con G2 hecho, el nativo usa el mismo camino. Cierra el caso general de `std/sort`.

### H4. `fs.remove_all(path)` y `fs.temp_dir()` / `fs.make_temp_dir(prefix)` (entrada 26) — S

Tres funciones en `std/fs`, con primitivos `__remove_all`, `__temp_dir` y `__make_temp_dir`
(directorio único por proceso: nada de colisiones entre ejecuciones en paralelo). Windows
incluido (`%TEMP%`).

## 4. Lote I — herramientas, mensajes y documentación (S, un solo hito)

- **`ray check`** como alias de `ray build` (entrada 10), y un subcomando desconocido dice
  "unknown subcommand 'check'; run `ray help`" en vez de `could not read module 'check'`.
- **`ray doc std/<módulo>`** y `ray doc <módulo>.<símbolo>` desde el CLI (entrada 7): la misma
  resolución que `ray_doc` del MCP.
- **Headless de `std/ui`** (entradas 8 y 9): `RAY_UI_TRACE=1` escribe en stderr cada `reply` y
  cada `eval_js` (`[ui] reply <window> <id> <json>` / `[ui] eval <window> <js>`), y
  `RAY_UI_EXIT_AFTER_MS=N` termina el proceso N ms después del último evento inyectado. Sirve a
  cualquier app de `std/ui` en CI, sin `perl -e 'alarm'`.
- **Profundidad de recursión** (entrada 20): `RAYLANG_MAX_DEPTH=N` en VM e intérprete
  (documentado con el valor por defecto) y el mensaje de `stack overflow` dice el límite vigente.
- **Mensajes** (entradas 22 y 24): `fn u32(…)` → `"'u32' is a primitive type and cannot name a
  function"`; una llamada dentro de un módulo que resuelve a la función propia del módulo cuyo
  nombre es un builtin, con tipo de argumento incompatible, añade `"(the module's own 'get'
  shadows the builtin here; use builtin.get(…) for the builtin)"`.
- **Documentación** (entradas 11, 23, 29, 33): en la REFERENCE, `{ }` y la línea en blanco
  entre `const` como forma canónica de `ray fmt`; `s[i]` con su coste y la regla `chars()` vs
  `bytes`; qué quita `trim()` (espacios, tabuladores y saltos de línea); `std/collections/set`
  para "conjunto de estados vistos" (entrada 33).
- **Nota de flujo** (entrada 31): en `docs/mcp.md` y `llms.txt`, "formatear al final, nunca
  entre parches; verificar con `grep` que el parche entró". La forma `} else` + espacios + `if`
  no se reproduce con 1.10.0: si vuelve a aparecer, hace falta el fragmento exacto.

## 5. Lote J — formatos en `std` (entradas 4, 5, 21)

Medido por la app: 2 648 archivos de configuración, ninguno legible con la stdlib tal cual.

### J1. `std/json` relajado (comentarios `//` `/* */` y comas finales) — S

`json.parse_relaxed(s)` (o `json.parse_with(s, JsonOptions)`): el mismo dialecto de
`tsconfig.json`, `.vscode/*.json`, `.eslintrc`. 1 876 de los 2 648 archivos.

### J2. `std/zip` de lectura — S–M

`std/inflate` ya hace lo difícil; falta el contenedor: directorio central, cabeceras locales,
`entries(z) -> [Entry]`, `read(z, name) -> Result<bytes, string>`, STORE y DEFLATE. Unas 200
líneas en raylang puro, y la app ya tiene el código y los 98 archivos de prueba.

### J3. `std/yaml` (lectura, subconjunto) y `std/plist` (XML) — L, a decidir

Los `.sublime-syntax` usan un subconjunto de YAML (mapas, listas, escalares, bloques `|`, anclas
raras). Un `std/yaml` completo es un proyecto; el subconjunto que lee la app cabe en 400 líneas.
`std/plist` es XML llano con un modelo de datos fijo; conviene junto a un `std/xml` mínimo. Se
decide cuando J1 y J2 estén: la app ya los tiene en `src/formats/` y no bloquean nada.

## 6. Lote K — decisiones que reabre la app (las toma el usuario)

### K1. `return` (y `panic`) como expresión de tipo `never` (entrada 13)

`Option.None => return code,` en un brazo de `match` es error de sintaxis. Hoy `return` es una
sentencia; la SPEC ya tiene la regla de divergencia para bloques (`{ return x; }` en un brazo
funciona). La forma mínima: `return e` como expresión de tipo `never` que unifica con
cualquiera, solo en posición de brazo o de `else`. Coste M: parser, checker (divergencia ya
existe), fmt, selfhost. Opción cero: documentar el rodeo `=> { return code; }`, que ya compila.

### K2. Copia estructural de structs (entrada 14)

Los structs son referencias; copiar es escribir cada campo. Opción: `@derive(Clone)` que genera
`clone(self) -> Self` con copia superficial (campos por valor; arrays y mapas compartidos, que
es lo que la app quiere para "guardar y restaurar flags"). Coste S–M: es el mismo mecanismo de
`@derive(Eq, Show)`, más selfhost y nativo. Alternativa: `copy(x)` builtin genérico. Recomendado
`@derive(Clone)`: explícito y sin sorpresas en structs con handles.

### K3. Intérprete embebible (entrada 6)

`ray.eval(source) -> Result<Value, string>` sobre la VM con funciones del host expuestas: es lo
que permitiría escribir extensiones del editor en raylang y correrlas como fibras. Es un arco
propio (API de valores, aislamiento, fuel/heap por evaluación, tipado del borde) y afecta a
`raycode`, servidores con hooks y bots. Fuera de este plan; se propone como §88 de IDEAS con
diseño aparte.

### K4. Trazas sin editar el código (entrada 30)

`@debug` que compile a nada en release, o `RAYLANG_TRACE=fn1,fn2` que imprima entradas y salidas
de esas funciones. Lo segundo no toca el lenguaje (es del runtime, VM e intérprete) y resuelve
el caso de "el bug se ve en el paso 7 de 5 000". Coste M. A decidir junto a K1–K2.

## 7. Datos que deja la app (sin trabajo asociado)

- **Brecha VM/nativo** (entradas 16, 19, 25): 26× en el bucle del resaltador, **121×** indexando
  paquetes (Map + bytes + E/S). La brecha no es una constante: depende de la mezcla. Va a
  PERFORMANCE.md como medición de referencia; el consejo "desarrolla contra el nativo cuando el
  bucle es de teclado" va al MANUAL de `std/ui`.
- **Corpus real** (entrada 15): 6 226 patrones únicos de 482 sintaxis; 39 % usan look-around.
  Es el argumento de G1: `std/regex` no puede ser el motor de un editor, y debe decirlo.
- **Prefiltro por primer carácter** (entrada 28): 95× en el motor propio de la app. `std/regex`
  se apoya en el crate `regex`, que ya lo hace; no aplica.
- **Medir antes de corregir** (entradas 30 y 32): agregados y volcados antes de la primera
  corrección. Lección de método; K4 es la parte que toca a la toolchain.

## 8. Orden propuesto y numeración

| Hito | Contenido | Efecto en `ray-sublime` |
|---|---|---|
| M211 | G1 (`std/regex` rechaza lo no soportado; ICE → error; pánico en worker → aborta) | ningún cuelgue; el corpus documenta lo que `std` no cubre |
| M212 | G2 (`impl Ord` + `sort` en nativo) | vuelve la ordenación por `Ord` sin inserción a mano |
| M213 | H1 (índice de string amortizado) | bucles sobre texto sin `chars()` previo |
| M214 | H2 (`Option.None` infiere del parámetro) | listas de opcionales sin variable auxiliar |
| M215 | H3 (`sort_by`, `sort_by_key`) | orden por clave en presentación |
| M216 | H4 (`fs.remove_all`, `temp_dir`) | suite de tests sin `remove_tree` propio |
| M217 | Lote I (herramientas, mensajes, docs) | CI headless sin `perl -e alarm`; `ray check` |
| M218 | J1 (`json.parse_relaxed`) | `src/formats/json_relaxed.ray` desaparece |
| M219 | J2 (`std/zip` lectura) | `src/formats/zip.ray` desaparece |
| J3, K | yaml/plist, `return` expresión, `@derive(Clone)`, trazas, intérprete embebible | según decisión |

Cada hito: rama + PR, SPEC antes si cambia el lenguaje (K1, K2), DESIGN con el porqué, CHANGELOG
"Sin publicar", y la validación final es el diff en `ray-sublime` que borra el rodeo.
