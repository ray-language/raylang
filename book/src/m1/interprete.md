# El intérprete

Hasta aquí todo el front-end *analiza* pero nada *ejecuta*. El **intérprete** cambia
eso: recorre el AST ya verificado y lo evalúa nodo a nodo, produciendo resultados de
verdad. Es la técnica más directa de ejecución, llamada *tree-walking* (caminar el
árbol), y con ella raylang por fin *corre*: `factorial(5)` imprime `120`.

Como el checker ya garantizó que el programa está bien tipado, el intérprete
**confía**: no re-verifica tipos. Las combinaciones imposibles se marcan con
`unreachable!` —si alguna saltara, sería un bug del checker, no del programa del
usuario. Esa división de responsabilidades mantiene el evaluador limpio.

## Idea 1: valores en runtime

El checker razonaba sobre `Type` en estático. El intérprete manipula su primo
dinámico, `Value`: lo que las expresiones *producen* al ejecutarse.

```rust
pub enum Value {
    Int(i64), Float(f64), Bool(bool), Str(String), Unit,
}
```

Evaluar un literal produce su `Value`; evaluar `a + b` evalúa ambos lados y combina
los valores. La distinción tipo/valor es sutil pero fundamental: el tipo es lo que
*sabíamos* del programa antes de correrlo; el valor es lo que *obtenemos* al
correrlo.

## Idea 2: el entorno con marcos y el scoping léxico

Las variables viven en una pila de ámbitos, como en el checker, pero ahora guardan
*valores*. La parte sutil es qué pasa en una **llamada a función**.

Cuando `main` llama a `g`, ¿debería `g` ver las variables locales de `main`? En un
lenguaje con *scoping léxico*, **no**: cada función solo ve sus parámetros y sus
propias locales. Para lograrlo, una llamada arranca con una pila de ámbitos
**nueva**, no la de quien llama. Guardamos la pila actual y la restauramos al
volver:

```rust
fn call_function(&mut self, func: &Function, args: Vec<Value>) -> EvalResult {
    let saved = mem::take(&mut self.scopes);   // guarda el entorno del llamador
    self.scopes.push(HashMap::new());          // ámbito base: los parámetros
    for (param, arg) in func.params.iter().zip(args) {
        self.define(&param.name, arg);
    }
    let result = self.exec_block(&func.body);
    self.scopes = saved;                        // restaura el del llamador
    // ...
}
```

Si en vez de esto empujáramos los ámbitos sobre la misma pila, la función vería las
variables de quien la llamó: eso sería *scoping dinámico*, un bug clásico. El test
correspondiente lo verifica: una función con un parámetro `x` usa *su* `x`, no el de
`main`.

## Idea 3: `return` como señal de flujo

Aquí está el problema más fino. Un `return` dentro de tres bloques anidados debe
abandonarlos todos de golpe. En un intérprete de árbol, eso es un **salto no-local**.
¿Cómo se implementa?

El truco clásico: reusar el canal de error de `Result`. Una evaluación puede
terminar normalmente con un `Value`, o interrumpirse con un `Flow`:

```rust
enum Flow {
    Return(Value),       // un 'return' propagándose hacia el borde de la función
    Error(RuntimeError), // un error de ejecución real, propagándose hasta el tope
}
type EvalResult = Result<Value, Flow>;
```

Cuando se ejecuta un `return e`, se lanza `Err(Flow::Return(v))`. Con el operador
`?`, ese error se propaga hacia arriba —saliendo de bloques y bucles— hasta que
`call_function` lo **atrapa** y lo convierte en el valor de retorno de la función:

```rust
match result {
    Ok(v)                  => Ok(v),   // el cuerpo cayó hasta su valor final
    Err(Flow::Return(v))   => Ok(v),   // un 'return' temprano: ese es el valor
    Err(e @ Flow::Error(_)) => Err(e), // un error real sigue subiendo
}
```

El mismo mecanismo transporta los errores de ejecución (como una división por cero)
hasta lo más alto. Es un patrón elegante: dos clases de "salir de aquí" viajando por
el mismo canal, distinguidas solo en el borde de la función.

## Un detalle: el cortocircuito

Los operadores `&&` y `||` no evalúan su lado derecho si el izquierdo ya decide el
resultado. Por eso se tratan aparte del resto de operadores binarios:

```rust
And => {
    if !self.eval_bool(left)? { return Ok(Value::Bool(false)); } // no toca la derecha
    Ok(Value::Bool(self.eval_bool(right)?))
}
```

Esto no es solo una optimización: cambia la semántica. `false && (1 / 0 == 0)`
devuelve `false` **sin** reventar, porque la división por cero de la derecha nunca
se evalúa.

## Errores de ejecución

Aunque el checker elimina los errores de tipo, quedan errores que solo se conocen al
correr: la división (o el módulo) por cero. El intérprete los reporta con ubicación,
igual que todas las fases:

```
error en ejecución en 3:5: división entera por cero
```

## El CLI se vuelve un runner

Con el intérprete, el binario de raylang deja de ser una herramienta de inspección y
se vuelve un **ejecutor** de verdad: lexea, parsea, verifica tipos y **ejecuta**. El
código de salida del proceso es el entero que devuelve `main`, como en C. Correr un
programa es, por fin:

```sh
cargo run --quiet -- examples/fib.ray
```

## M1 completo

Con el intérprete cerramos el primer hito. raylang recorre la tubería entera —léxico,
sintaxis, semántica y evaluación— y ejecuta programas reales: recursión, bucles,
mutación, condicionales, funciones. Es pequeño, pero es un lenguaje *de verdad*.

## Lo que sigue

El intérprete tree-walking es correcto pero lento: re-recorre el árbol en cada
evaluación. El siguiente hito, **M2**, reescribe el motor de ejecución como
**bytecode + máquina virtual**, reutilizando intacto todo el front-end. Y el
intérprete que acabamos de construir será nuestro **oráculo**: correremos los mismos
programas en ambos y compararemos resultados, para asegurarnos de que la VM hace
exactamente lo mismo.

> El código de esta fase vive en `src/interpreter.rs`.
