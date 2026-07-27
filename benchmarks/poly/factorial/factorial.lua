function fact(n)
  if n < 2 then return 1 end
  return n * fact(n - 1)
end

function main()
  local _t0 = os.clock()
  for i = 0, 9 do
    print(fact(i))
  end
  io.stderr:write(string.format("bench_ns=%.0f\n", (os.clock() - _t0) * 1e9))
end

main()
