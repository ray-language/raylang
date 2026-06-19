# El checker

> 🚧 **Capítulo en construcción.** Se escribirá al consolidar esta fase.

Cubrirá: el **análisis semántico** y la verificación de tipos; las **dos pasadas**
(registrar firmas de funciones para permitir recursión y llamadas hacia adelante, y
luego verificar los cuerpos); la **pila de ámbitos** que da *shadowing* léxico; las
reglas de tipos (sin conversiones implícitas, condiciones booleanas, ramas del `if`
del mismo tipo); y el **análisis de divergencia** que permite el retorno implícito.

> Código: `src/checker.rs`.
