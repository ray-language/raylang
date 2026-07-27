local function main()
    local _t0 = os.clock()
    local n = 200
    local a, b, c = {}, {}, {}
    for i = 1, n do
        a[i] = {}
        b[i] = {}
        c[i] = {}
        for j = 1, n do
            a[i][j] = (i - 1) * n + (j - 1)
            a[i][j] = a[i][j] % 13
            b[i][j] = ((j - 1) * n + (i - 1)) % 17
            c[i][j] = 0.0
        end
    end

    for i = 1, n do
        for j = 1, n do
            local s = 0.0
            for k = 1, n do
                s = s + a[i][k] * b[k][j]
            end
            c[i][j] = s
        end
    end

    local checksum = 0.0
    for i = 1, n do
        for j = 1, n do
            checksum = checksum + c[i][j]
        end
    end

    print(math.floor(checksum))
    io.stderr:write(string.format("bench_ns=%.0f\n", (os.clock() - _t0) * 1e9))
end

main()
