import sys
import time


def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)


def main():
    _t0 = time.perf_counter_ns()
    print(fib(34))
    print(f"bench_ns={time.perf_counter_ns() - _t0}", file=sys.stderr)


if __name__ == "__main__":
    sys.setrecursionlimit(10000)
    main()
