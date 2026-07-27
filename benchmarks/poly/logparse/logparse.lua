local _t0 = os.clock()
local statuses = {"200", "200", "200", "404", "500"}
local cnt = {}
local lat = {}
for i = 0, 150000 - 1 do
    local path = "/api/" .. (i % 50)
    local status = statuses[(i % 5) + 1]
    local line = "GET " .. path .. " " .. status .. " " .. (i % 250)
    local f = {}
    for w in string.gmatch(line, "%S+") do f[#f + 1] = w end
    cnt[f[3]] = (cnt[f[3]] or 0) + 1
    lat[f[2]] = (lat[f[2]] or 0) + tonumber(f[4])
end
local keys = {}
for k in pairs(cnt) do keys[#keys + 1] = k end
table.sort(keys)
for _, k in ipairs(keys) do print(k .. " " .. cnt[k]) end
local total = 0
for _, v in pairs(lat) do total = total + v end
print(total)
io.stderr:write(string.format("bench_ns=%.0f\n", (os.clock() - _t0) * 1e9))
