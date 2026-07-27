import sys, time


def fact(n):
    if n < 2:
        return 1
    return n * fact(n - 1)


def main():
    _t0 = time.perf_counter_ns()
    for i in range(10):
        print(fact(i))
    print(f"bench_ns={time.perf_counter_ns() - _t0}", file=sys.stderr)


if __name__ == "__main__":
    main()
