import sys, time


def main():
    _t0 = time.perf_counter_ns()
    n = 400000
    checksum = 0
    total_name_len = 0
    for i in range(n):
        line = '{"id":' + str(i) + ',"name":"user' + str(i) + '","score":' + str(i % 100) + '}'

        id_start = line.index(":") + 1
        id_end = line.index(",")
        id_val = int(line[id_start:id_end])

        name_start = line.index('"name":"') + len('"name":"')
        name_end = line.index('"', name_start)
        name_val = line[name_start:name_end]

        score_start = line.rindex(":") + 1
        score_end = line.rindex("}")
        score_val = int(line[score_start:score_end])

        checksum = (checksum * 31 + id_val + score_val) % 1000000007
        total_name_len += len(name_val)

    print(checksum)
    print(total_name_len)
    print(f"bench_ns={time.perf_counter_ns() - _t0}", file=sys.stderr)


if __name__ == "__main__":
    main()
