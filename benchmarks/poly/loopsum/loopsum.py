import sys, time


def main():
    _t0 = time.perf_counter_ns()
    acc = 0
    for i in range(1, 10000001):
        acc = (acc + i * i) % 1000000007
    print(acc)
    print(f"bench_ns={time.perf_counter_ns() - _t0}", file=sys.stderr)


if __name__ == "__main__":
    main()
