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

## Resultado de referencia

En `fib(32)`, la VM corre **~3.2x más rápido** que el intérprete (~550 ms vs ~1.76 s),
con mucha menos varianza. Ese ~3.2× es tras las optimizaciones **Opt.1** (no clonar la
instrucción por iteración) y **Opt.2** (pool de locales por llamada); el punto de partida
era ~2.4× (ver el registro **medido** en `IDEAS.md` §11, incl. optimizaciones evaluadas y
descartadas como `Rc<str>`). El *por qué* y las pendientes están en `IDEAS.md`; la
narrativa, en el libro (capítulo de optimización de la VM).
