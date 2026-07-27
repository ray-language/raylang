local _t0 = os.clock()
local parts = {}
for i = 0, 400000 - 1 do
    parts[#parts + 1] = '{"id":' .. i .. ',"name":"user' .. i .. '","score":' .. (i % 100) .. '}'
end
local out = table.concat(parts, "\n")
print(#out)
print(#parts)
io.stderr:write(string.format("bench_ns=%.0f\n", (os.clock() - _t0) * 1e9))
