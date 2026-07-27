import sys, time


def main():
    _t0 = time.perf_counter_ns()
    n = 200
    a = [[float((i * n + j) % 13) for j in range(n)] for i in range(n)]
    b = [[float((j * n + i) % 17) for j in range(n)] for i in range(n)]
    c = [[0.0] * n for _ in range(n)]

    for i in range(n):
        for j in range(n):
            s = 0.0
            for k in range(n):
                s += a[i][k] * b[k][j]
            c[i][j] = s

    checksum = 0.0
    for i in range(n):
        for j in range(n):
            checksum += c[i][j]

    print(int(checksum))
    print(f"bench_ns={time.perf_counter_ns() - _t0}", file=sys.stderr)


if __name__ == "__main__":
    main()
