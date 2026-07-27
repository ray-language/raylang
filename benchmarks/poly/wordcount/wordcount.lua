local _t0 = os.clock()
local base = "the quick brown fox jumps over the lazy dog and runs away fast today"
local m = {}
for r = 0, 120000 - 1 do
    local line = tostring(r % 1000) .. " " .. base
    for w in string.gmatch(line, "%S+") do
        m[w] = (m[w] or 0) + 1
    end
end
local keys = {}
for k in pairs(m) do keys[#keys + 1] = k end
table.sort(keys)
local acc = 0
for _, k in ipairs(keys) do
    acc = (acc * 31 + m[k]) % 1000000007
end
print(#keys)
print(acc)
io.stderr:write(string.format("bench_ns=%.0f\n", (os.clock() - _t0) * 1e9))
