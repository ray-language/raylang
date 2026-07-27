import re
import sys, time


def main():
    _t0 = time.perf_counter_ns()
    n = 200000
    pattern = re.compile(r"user(\d+) GET /api/(\d+) (\d+) (\d+)ms")
    checksum = 0
    match_count = 0
    for i in range(n):
        status = 200 if i % 5 != 4 else 404
        line = f"user{i} GET /api/{i % 50} {status} {i % 250}ms"
        m = pattern.match(line)
        if m:
            match_count += 1
            uid = int(m.group(1))
            path = int(m.group(2))
            st = int(m.group(3))
            ms = int(m.group(4))
            checksum = (checksum * 31 + uid + path + st + ms) % 1000000007

    print(match_count)
    print(checksum)
    print(f"bench_ns={time.perf_counter_ns() - _t0}", file=sys.stderr)


if __name__ == "__main__":
    main()
