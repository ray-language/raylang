local function main()
    local _t0 = os.clock()
    local n = 200000
    local checksum = 0
    local match_count = 0

    for i = 0, n - 1 do
        local status = (i % 5 ~= 4) and 200 or 404
        local line = "user" .. i .. " GET /api/" .. (i % 50) .. " " .. status .. " " .. (i % 250) .. "ms"

        local uid, path, st, ms = line:match("^user(%d+) GET /api/(%d+) (%d+) (%d+)ms$")
        if uid then
            match_count = match_count + 1
            checksum = (checksum * 31 + tonumber(uid) + tonumber(path) + tonumber(st) + tonumber(ms)) % 1000000007
        end
    end

    print(match_count)
    print(checksum)
    io.stderr:write(string.format("bench_ns=%.0f\n", (os.clock() - _t0) * 1e9))
end

main()
