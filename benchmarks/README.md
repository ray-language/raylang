# Benchmarks

Comparan los dos motores de ejecución de raylang: el **intérprete** (M1) y la
**máquina virtual** (M2). Sirven para medir el progreso a medida que optimizamos la
VM (ver las ideas de optimización en `IDEAS.md`).

## Uso

```sh
benchmarks/bench.sh                 # programa por defecto (benchmarks/fib.ray)
benchmarks/bench.sh otro.ray        # otro programa
benchmarks/bench.sh fib.ray -r 20   # args extra que se pasan a hyperfine
```

El script compila en modo release automáticamente y requiere
[hyperfine](https://github.com/sharkdp/hyperfine).

## Resultado de referencia

En `fib(32)`, la VM corre **~3x más rápido** que el intérprete, y con mucha menos
varianza (las locales por índice evitan la asignación de un `HashMap` por llamada).
El *por qué* y las optimizaciones pendientes están en `IDEAS.md`; la narrativa, en
el libro (sección M2).
