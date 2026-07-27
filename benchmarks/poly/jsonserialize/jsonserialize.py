import sys, time
_t0 = time.perf_counter_ns()
parts = []
for i in range(400000):
    parts.append('{"id":' + str(i) + ',"name":"user' + str(i) + '","score":' + str(i % 100) + '}')
out = "\n".join(parts)
print(len(out))
print(len(parts))
print(f"bench_ns={time.perf_counter_ns() - _t0}", file=sys.stderr)
