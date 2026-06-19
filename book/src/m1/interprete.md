# El intérprete

> 🚧 **Capítulo en construcción.** Se escribirá al consolidar esta fase.

Cubrirá: el **intérprete tree-walking** que por fin *ejecuta*; los **valores en
runtime** (el primo dinámico de `Type`); el **entorno con marcos de llamada** que da
*scoping léxico* (una función no ve las variables de quien la llamó); el `return`
modelado como una **señal de flujo** que se propaga hasta el borde de la función; el
**cortocircuito** de `&&`/`||`; y los errores de ejecución con ubicación.

> Código: `src/interpreter.rs`.
