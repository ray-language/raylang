# Gate de regresión de rendimiento (M35c) y de MEMORIA (M98.4). Mide los casos del banco sobre
# la VM de release (tiempo: mejor-de-N; memoria: pico de RSS por hijo vía os.wait4/ru_maxrss) y
# los compara contra un baseline COMMITEADO (`benchmarks/baseline.json`). Falla (exit 1) si algún
# caso excede su umbral (tiempo >5%, memoria >15%). Solo necesita python3 (invariante cero-deps).
#
# Uso:
#   python3 benchmarks/regress.py --record     # graba el baseline en esta máquina
#   python3 benchmarks/regress.py              # comprueba contra el baseline (gate)
#   python3 benchmarks/regress.py --threshold 0.08   # umbral a medida (default 5%)
#   python3 benchmarks/regress.py --strict     # aplica el gate aunque la máquina no case
#
# LA HUELLA DE MÁQUINA. Los tiempos absolutos dependen del hardware, así que un baseline
# grabado en la máquina A no es comparable en la B. El baseline guarda una huella
# (plataforma + nº de CPUs + modelo); si la de ahora NO casa, el gate degrada a
# **informativo** (avisa y sale 0) salvo con `--strict`. En CI —mismo runner siempre— la
# huella casa y el gate es estricto, que es justo lo que M35c pide.

import json
import os
import platform
import subprocess
import sys
import time

BIN = "./target/release/raylang"
BASELINE = "benchmarks/baseline.json"
DEFAULT_THRESHOLD = 0.05  # 5% (PRODUCCION.md M35c)
N = 15  # mejor-de-N; N alto filtra mejor el ruido del planificador (measure.py halló que 15 hace falta)

# Los casos del banco (los mismos que measure.py), sobre la VM de release.
CASES = [
    ("fib35",    [BIN, "--vm", "benchmarks/fib35.ray"]),
    ("loop10M",  [BIN, "--vm", "benchmarks/loop.ray"]),
    ("arrays",   [BIN, "--vm", "benchmarks/arrays.ray"]),
    ("gcnested", [BIN, "--vm", "benchmarks/gcnested.ray"]),
]

# M98.4: los casos de MEMORIA — pico de RSS (ru_maxrss del hijo, vía os.wait4; cero deps).
# Vigilan las fugas cazadas en docs/investigacion-uso-de-memoria.md: el churn de tareas (M98.1),
# el de canales (M98.3) y el coste de residencia de los arreglos de escalares (§4 / M98.5).
# Concurrencia → --deterministic (RSS reproducible; el orden de fibras no cambia la residencia
# pero sí el ruido). Umbral propio MEM_THRESHOLD: el RSS es estable pero cuantizado en páginas
# y sensible al allocator → 15% (una fuga real lo revienta por múltiplos, no por porcentajes).
MEM_CASES = [
    ("task_churn", [BIN, "--vm", "--deterministic", "benchmarks/task_churn.ray"]),
    ("chan_churn", [BIN, "--vm", "--deterministic", "benchmarks/chan_churn.ray"]),
    ("arr_1M",     [BIN, "--vm", "benchmarks/arr_while.ray"]),
]
MEM_THRESHOLD = 0.15
MEM_N = 3  # el RSS varía poco entre corridas; mejor-de-3 basta


def rss_child(cmd):
    """Corre `cmd` y devuelve su pico de RSS en BYTES (ru_maxrss por hijo, vía os.wait4).
    macOS lo reporta en bytes; Linux en KiB → se normaliza."""
    p = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    _, status, ru = os.wait4(p.pid, 0)
    p.returncode = status  # evita el warning de Popen sin wait
    if os.WIFSIGNALED(status):
        sys.exit(f"'{' '.join(cmd)}' murió por señal {os.WTERMSIG(status)}")
    maxrss = ru.ru_maxrss
    return maxrss if sys.platform == "darwin" else maxrss * 1024


def medir_mem():
    """Mejor-de-MEM_N (mínimo) por caso de memoria; devuelve {nombre: bytes}."""
    if not os.path.exists(BIN):
        sys.exit(f"no existe {BIN}; compila primero: cargo build --release")
    picos = {}
    for nombre, cmd in MEM_CASES:
        picos[nombre] = min(rss_child(cmd) for _ in range(MEM_N))
    return picos


def mb(n):
    return f"{n / 1048576:.1f} MB"


def fingerprint():
    """Identifica la clase de máquina: los tiempos solo son comparables dentro de una."""
    modelo = ""
    try:
        if sys.platform == "darwin":
            modelo = subprocess.check_output(
                ["sysctl", "-n", "machdep.cpu.brand_string"], text=True
            ).strip()
        elif sys.platform.startswith("linux"):
            with open("/proc/cpuinfo") as f:
                for linea in f:
                    if linea.startswith("model name"):
                        modelo = linea.split(":", 1)[1].strip()
                        break
    except Exception:
        pass
    return f"{platform.system()}-{platform.machine()}-cpus{os.cpu_count()}-{modelo}"


def medir():
    """Mejor-de-N por caso; devuelve {nombre: segundos}. Aborta si el binario no está."""
    if not os.path.exists(BIN):
        sys.exit(f"no existe {BIN}; compila primero: cargo build --release")
    tiempos = {}
    for nombre, cmd in CASES:
        best = float("inf")
        for _ in range(N):
            ini = time.perf_counter()
            r = subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            # Los benchmarks devuelven su cómputo como código de salida (`main -> int`, & 0xFF),
            # así que un código no-cero es NORMAL (fib35 → 201). Solo un código negativo indica
            # que el proceso murió por una señal (crash real) — eso sí aborta.
            if r.returncode < 0:
                sys.exit(f"el caso '{nombre}' murió por señal {-r.returncode}")
            best = min(best, time.perf_counter() - ini)
        tiempos[nombre] = best
    return tiempos


def record():
    datos = {"fingerprint": fingerprint(), "n": N, "cases": medir(), "mem": medir_mem()}
    with open(BASELINE, "w") as f:
        json.dump(datos, f, indent=2)
        f.write("\n")
    print(f"baseline grabado en {BASELINE} ({datos['fingerprint']})")
    for nombre, seg in datos["cases"].items():
        print(f"  {nombre:10s} {seg:.4f} s")
    for nombre, b in datos["mem"].items():
        print(f"  {nombre:10s} {mb(b)}")


def check(threshold, strict):
    if not os.path.exists(BASELINE):
        sys.exit(f"no hay baseline ({BASELINE}); grábalo con: python3 benchmarks/regress.py --record")
    with open(BASELINE) as f:
        base = json.load(f)
    ahora_fp = fingerprint()
    misma_maquina = ahora_fp == base["fingerprint"]
    if not misma_maquina and not strict:
        print("AVISO: la máquina no casa con la del baseline — el gate es solo informativo.")
        print(f"  baseline: {base['fingerprint']}")
        print(f"  ahora:    {ahora_fp}")
    actual = medir()
    print(f"=== regresión de rendimiento (umbral {threshold:.0%}, mejor de {N}) ===")
    peor = 0.0
    regresiones = []
    for nombre, seg in actual.items():
        ref = base["cases"].get(nombre)
        if ref is None:
            print(f"  {nombre:10s} {seg:.4f} s   (nuevo; sin referencia)")
            continue
        delta = seg / ref - 1.0
        peor = max(peor, delta)
        marca = "OK  " if delta <= threshold else "LENTO"
        print(f"  {nombre:10s} {seg:.4f} s  vs {ref:.4f} s  ({delta:+.1%})  {marca}")
        if delta > threshold:
            regresiones.append((nombre, delta))
    # M98.4: el gate de MEMORIA (pico de RSS vs baseline; umbral propio, más ancho).
    base_mem = base.get("mem")
    if base_mem is None:
        print("\n(baseline sin sección 'mem' — re-graba con --record para activar el gate de memoria)")
    else:
        actual_mem = medir_mem()
        print(f"=== regresión de memoria (umbral {MEM_THRESHOLD:.0%}, pico de RSS, mejor de {MEM_N}) ===")
        for nombre, b in actual_mem.items():
            ref = base_mem.get(nombre)
            if ref is None:
                print(f"  {nombre:10s} {mb(b)}   (nuevo; sin referencia)")
                continue
            delta = b / ref - 1.0
            peor = max(peor, delta)
            marca = "OK  " if delta <= MEM_THRESHOLD else "GORDO"
            print(f"  {nombre:10s} {mb(b)}  vs {mb(ref)}  ({delta:+.1%})  {marca}")
            if delta > MEM_THRESHOLD:
                regresiones.append((nombre, delta))
    # Veredicto: el gate solo es duro si la máquina casa (o --strict).
    if regresiones and (misma_maquina or strict):
        print(f"\nREGRESIÓN: {len(regresiones)} caso(s) fuera de umbral (tiempo o memoria) → falla.")
        sys.exit(1)
    if regresiones:
        print(f"\n(informativo) {len(regresiones)} caso(s) fuera de umbral, pero la máquina no casa.")
    else:
        print(f"\nsin regresión (peor caso {peor:+.1%}).")
    sys.exit(0)


def main():
    args = sys.argv[1:]
    if "--record" in args:
        record()
        return
    threshold = DEFAULT_THRESHOLD
    if "--threshold" in args:
        threshold = float(args[args.index("--threshold") + 1])
    check(threshold, strict="--strict" in args)


if __name__ == "__main__":
    main()
