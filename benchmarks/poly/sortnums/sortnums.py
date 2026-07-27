import sys, time


def main():
    _t0 = time.perf_counter_ns()
    n = 1000000
    seed = 12345
    arr = []
    for _ in range(n):
        seed = (48271 * seed) % 2147483647
        arr.append(seed % 1000000)

    arr.sort()

    checksum = 0
    for v in arr:
        checksum = (checksum * 31 + v) % 1000000007

    print(arr[0])
    print(arr[-1])
    print(checksum)
    print(f"bench_ns={time.perf_counter_ns() - _t0}", file=sys.stderr)


if __name__ == "__main__":
    main()
