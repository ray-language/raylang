local function main()
    local _t0 = os.clock()
    local n = 1000000
    local seed = 12345
    local arr = {}
    for i = 1, n do
        seed = (48271 * seed) % 2147483647
        arr[i] = seed % 1000000
    end

    table.sort(arr)

    local checksum = 0
    for i = 1, n do
        checksum = (checksum * 31 + arr[i]) % 1000000007
    end

    print(arr[1])
    print(arr[n])
    print(checksum)
    io.stderr:write(string.format("bench_ns=%.0f\n", (os.clock() - _t0) * 1e9))
end

main()
