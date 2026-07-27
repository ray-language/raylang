function fib(n)
  if n < 2 then return n end
  return fib(n - 1) + fib(n - 2)
end

function main()
  local _t0 = os.clock()
  for i = 0, 9 do
    print(fib(i))
  end
  io.stderr:write(string.format("bench_ns=%.0f\n", (os.clock() - _t0) * 1e9))
end

main()
