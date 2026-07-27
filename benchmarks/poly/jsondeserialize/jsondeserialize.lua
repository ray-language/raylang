local function main()
    local _t0 = os.clock()
    local n = 400000
    local checksum = 0
    local total_name_len = 0

    for i = 0, n - 1 do
        local line = '{"id":' .. i .. ',"name":"user' .. i .. '","score":' .. (i % 100) .. "}"

        local id_str, name_val, score_str = line:match('"id":(%d+),"name":"(user%d+)","score":(%d+)}')
        local id_val = tonumber(id_str)
        local score_val = tonumber(score_str)

        checksum = (checksum * 31 + id_val + score_val) % 1000000007
        total_name_len = total_name_len + #name_val
    end

    print(checksum)
    print(total_name_len)
    io.stderr:write(string.format("bench_ns=%.0f\n", (os.clock() - _t0) * 1e9))
end

main()
