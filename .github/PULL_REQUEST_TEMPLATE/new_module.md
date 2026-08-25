## Módulo nuevo: `<std/nombre | paquete/modulo>`

<!-- Qué problema resuelve y por qué merece vivir en el lenguaje (mejor un hallazgo de uso real
     que una idea especulativa; enlaza el §de IDEAS.md si existe). -->

## Decisiones de diseño

<!-- Forma de la API (errores como valores, módulos vs métodos, genéricos), qué queda FUERA a
     propósito, y alternativas consideradas. Si añade una dependencia de Cargo: justificación
     estilo SECURITY.md (qué clase de código evita escribir a mano). -->

## Batería de admisión (module_policy + guardas — todo debe estar en verde)

- [ ] **Raylang puro salvo justificación** (cero runtime nuevo; si necesita primitivo/crate, el porqué arriba)
- [ ] **Superficie pública documentada**: todo `pub` con `///` en inglés (regla 1)
- [ ] **Fila en REFERENCE.md** con firmas (regla 2) — y sección en MANUAL.md si tiene receta de uso
- [ ] **Tests dedicados en `tests/`** que lo ejercitan por AMBOS motores (regla 3/4); golden determinista
- [ ] **Identificadores en inglés, comentarios `//` en español** (`naming_policy`)
- [ ] **Forma canónica** (`ray fmt --write` + `fmt_policy`)
- [ ] Errores **como valores** (`Result`/`Option`), mensajes de error en INGLÉS
- [ ] Si es un paquete (`packages/<p>`): `README.md` + `ray.toml` con name/version; si es de `std/`: alta en `src/stdlib.rs`
- [ ] **Byte-identidad**: misma salida en intérprete, VM y (si aplica su superficie) binario nativo
- [ ] DESIGN.md gana su sección de crónica; CHANGELOG.md "Sin publicar"; IDEAS.md marcada EJECUTADA si cierra una

## Dogfood

<!-- Cómo se validó con uso real: app, ejemplo o test e2e. "Compila" no es dogfood. -->
