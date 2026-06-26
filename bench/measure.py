import subprocess, sys, time
BIN = "./target/release/raylang"
CASES = [
    ("fib(35)      VM",  [BIN, "--vm", "bench/fib35.ray"]),
    ("loop 10M     VM",  [BIN, "--vm", "bench/loop.ray"]),
    ("arrays 2000x VM",  [BIN, "--vm", "bench/arrays.ray"]),
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
