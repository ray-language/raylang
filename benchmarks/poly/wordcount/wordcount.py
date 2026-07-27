from collections import defaultdict
import sys, time
_t0 = time.perf_counter_ns()
base = "the quick brown fox jumps over the lazy dog and runs away fast today"
m = defaultdict(int)
for r in range(120000):
    line = str(r % 1000) + " " + base
    for w in line.split(" "):
        m[w] += 1
acc = 0
for k in sorted(m.keys()):
    acc = (acc * 31 + m[k]) % 1000000007
print(len(m))
print(acc)
print(f"bench_ns={time.perf_counter_ns() - _t0}", file=sys.stderr)
