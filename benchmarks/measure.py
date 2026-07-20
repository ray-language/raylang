# Arnés de medición SIN hyperfine (alternativa a bench.sh): corre cada caso N veces sobre el binario de
# release y reporta el MEJOR tiempo (filtra el ruido del planificador mejor que la media). Solo necesita
# python3. Ejecútalo desde la raíz del repo:  python3 benchmarks/measure.py "etiqueta"
import os, subprocess, sys, time
BIN = "./target/release/raylang"
CASES = [
    ("fib(35)      VM",  [BIN, "--vm", "benchmarks/fib35.ray"]),
    ("loop 10M     VM",  [BIN, "--vm", "benchmarks/loop.ray"]),
    ("arrays 2000x VM",  [BIN, "--vm", "benchmarks/arrays.ray"]),
    ("gcnested     VM",  [BIN, "--vm", "benchmarks/gcnested.ray"]),  # GC con hijos en el heap
]
# Mejor-de-N: N grande filtra mejor el ruido del planificador. Opt.7 (~8 %) quedaba enmascarado con N=5
# (la baseline saltaba ±4 % entre corridas) y se destapó limpio con N=15. Usa N alto para señales pequeñas.
N = 15
# M98.4: los casos de MEMORIA (pico de RSS del hijo, vía os.wait4 → ru_maxrss; cero deps).
# Son los mismos que el gate de regress.py; ver docs/investigacion-uso-de-memoria.md.
MEM_CASES = [
    ("task_churn 20k", [BIN, "--vm", "--deterministic", "benchmarks/task_churn.ray"]),
    ("chan_churn 20k", [BIN, "--vm", "--deterministic", "benchmarks/chan_churn.ray"]),
    ("arr 1M ints",    [BIN, "--vm", "benchmarks/arr_while.ray"]),
]
print(f"=== {sys.argv[1] if len(sys.argv)>1 else 'medición'} (mejor de {N}) ===")
for label, cmd in CASES:
    best = float("inf")
    for _ in range(N):
        s = time.perf_counter()
        subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        best = min(best, time.perf_counter() - s)
    print(f"{label:18s} {best:.4f} s")
print("--- memoria (pico de RSS, mejor de 3) ---")
for label, cmd in MEM_CASES:
    best = float("inf")
    for _ in range(3):
        p = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        _, status, ru = os.wait4(p.pid, 0)
        p.returncode = status
        rss = ru.ru_maxrss if sys.platform == "darwin" else ru.ru_maxrss * 1024
        best = min(best, rss)
    print(f"{label:18s} {best/1048576:.1f} MB")
