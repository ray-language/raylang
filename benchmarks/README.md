# Benchmarks

Dos arneses con trabajos distintos — no se solapan y ninguno reemplaza al otro:

| | Qué compara | Para qué |
|---|---|---|
| **este directorio** | raylang contra sí mismo (intérprete M1 vs VM M2, y VM contra un baseline commiteado) | **gate** de regresión: avisa si un cambio nos deja más lentos |
| **`poly/`** | raylang contra node/go/rust/php/lua/python/ruby/perl, en tiempo Y memoria | **comparación** externa: las cifras de `PERFORMANCE.md` §1/§6 salen de aquí |

`poly/` es un arnés en python3 puro (`./bench.py`, o `./tui.py` interactivo) que necesita los
compiladores/intérpretes de los otros lenguajes instalados —los que falten se omiten con un
aviso— y **no está cableado a CI**: es una herramienta de sesión, se corre a mano. Ver
`poly/README.md`, en particular su §Metodología de medición (rondas intercaladas con rotación,
mediana+MAD, detector de desalojo del SO), que es más rigurosa que la de `measure.py`.

El resto de este documento describe el arnés de **este** directorio: compara los dos motores de
ejecución de raylang, el **intérprete** (M1) y la **máquina virtual** (M2). Sirve para medir el
progreso a medida que optimizamos la VM (ver las ideas de optimización en `IDEAS.md`).

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

## Gate de regresión (M35c)

`regress.py` convierte el banco en un **gate**: mide la VM de release (mejor-de-15) y la
compara contra un baseline commiteado (`baseline.json`), **fallando (exit 1) si algún caso es
>5 % más lento**. Solo python3.

```sh
python3 benchmarks/regress.py --record          # graba el baseline en ESTA máquina
python3 benchmarks/regress.py                   # comprueba (el gate)
python3 benchmarks/regress.py --threshold 0.10  # umbral a medida
cargo test --release --test perf_regression -- --ignored --nocapture   # el mismo gate vía cargo
```

**La huella de máquina.** Los tiempos absolutos dependen del hardware, así que el baseline
guarda una huella (plataforma + CPUs + modelo). Si la de ahora **no casa**, el gate degrada a
**informativo** (avisa y sale 0) — así es seguro en cualquier máquina. El baseline commiteado se
grabó en un Apple M3 Pro; en **otra máquina o en CI, graba el tuyo** una vez (`--record`,
cacheado) y a partir de ahí el gate es estricto en ese runner (lo que M35c pide). `--strict`
fuerza el gate ignorando la huella.

**Por qué mejor-de-15 y 5 %.** A mejor-de-7 este banco tiene ~5 % de varianza entre corridas en
un portátil (el gate false-positivea); a **mejor-de-15** baja a ~1-1.5 %, dejando ~3.5 % de
holgura bajo el umbral del 5 %. Es la misma N que `measure.py` necesitó para destapar señales
pequeñas. En un runner de CI dedicado (más silencioso) el 5 % es holgado.

`iter.ray` (Opt.13) mide el camino **lazy** (`for x in xs.iter()` sobre 1M): es sensible al
**pacing del GC** (un contenedor grande vivo + umbral por conteo re-escaneaba el arreglo cada
~50 asignaciones → 6.8 s; con el umbral amortizado por trabajo trazado, 0.4 s).
