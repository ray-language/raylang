# Arnés de medición SIN hyperfine (alternativa a bench.sh): corre cada caso N veces sobre el binario de
# release y reporta el MEJOR tiempo (filtra el ruido del planificador mejor que la media). Solo necesita
# python3. Ejecútalo desde la raíz del repo:  python3 benchmarks/measure.py "etiqueta"
import subprocess, sys, time
BIN = "./target/release/raylang"
CASES = [
    ("fib(35)      VM",  [BIN, "--vm", "benchmarks/fib35.ray"]),
    ("loop 10M     VM",  [BIN, "--vm", "benchmarks/loop.ray"]),
    ("arrays 2000x VM",  [BIN, "--vm", "benchmarks/arrays.ray"]),
]
N = 5
print(f"=== {sys.argv[1] if len(sys.argv)>1 else 'medición'} (mejor de {N}) ===")
for label, cmd in CASES:
    best = float("inf")
    for _ in range(N):
        s = time.perf_counter()
        subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        best = min(best, time.perf_counter() - s)
    print(f"{label:18s} {best:.4f} s")
