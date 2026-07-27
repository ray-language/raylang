from collections import defaultdict
import sys, time
_t0 = time.perf_counter_ns()
statuses = ["200", "200", "200", "404", "500"]
cnt = defaultdict(int)
lat = defaultdict(int)
for i in range(150000):
    path = "/api/" + str(i % 50)
    status = statuses[i % 5]
    line = "GET " + path + " " + status + " " + str(i % 250)
    f = line.split(" ")
    cnt[f[2]] += 1
    lat[f[1]] += int(f[3])
for k in sorted(cnt.keys()):
    print(k + " " + str(cnt[k]))
print(sum(lat.values()))
print(f"bench_ns={time.perf_counter_ns() - _t0}", file=sys.stderr)
