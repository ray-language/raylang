# Benchmarks

Comparan los dos motores de ejecución de raylang: el **intérprete** (M1) y la
**máquina virtual** (M2). Sirven para medir el progreso a medida que optimizamos la
VM (ver las ideas de optimización en `IDEAS.md`).

## Uso

```sh
benchmarks/bench.sh                 # programa por defecto (benchmarks/fib.ray)
benchmarks/bench.sh otro.ray        # otro programa (p. ej. benchmarks/strings.ray)
benchmarks/bench.sh fib.ray -r 20   # args extra que se pasan a hyperfine
```

El script compila en modo release automáticamente y requiere
[hyperfine](https://github.com/sharkdp/hyperfine). Programas: `fib.ray` (recursión
intensa, mide el coste de llamada/despacho) y `strings.ray` (string-heavy, mide el
coste de mover/construir strings).

### Sin hyperfine

`measure.py` es una alternativa que **solo necesita python3** (mejor-de-N, sin deps externas):

```sh
python3 benchmarks/measure.py "etiqueta"   # mide fib35/loop/arreglos sobre la VM (release)
```

Programas extra: `fib35.ray` (recursión más larga), `loop.ray` (bucle aritmético apretado)
y `arrays.ray` (asignación en heap + GC).

## Resultado de referencia

En `fib(32)`, la VM corre **~3.2x más rápido** que el intérprete (~550 ms vs ~1.76 s),
con mucha menos varianza. Ese ~3.2× es tras las optimizaciones **Opt.1** (no clonar la
instrucción por iteración), **Opt.2** (pool de locales por llamada) y **Opt.4** (fast-path
entero en el lazo de ops binarias: fib(35) −5 %, bucle 10M −6 %); el punto de partida era
~2.4×. Entre medias, **Opt.3** (`Rc<str>`) se evaluó y **descartó** (medido, sin mejora), y
LTO/`codegen-units=1` también. El registro **medido** completo está en `IDEAS.md` §11; la
narrativa, en el libro (capítulo de optimización de la VM).
