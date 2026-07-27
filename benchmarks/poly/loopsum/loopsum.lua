function main()
  local _t0 = os.clock()
  local acc = 0
  for i = 1, 10000000 do
    acc = (acc + i * i) % 1000000007
  end
  print(acc)
  io.stderr:write(string.format("bench_ns=%.0f\n", (os.clock() - _t0) * 1e9))
end

main()
