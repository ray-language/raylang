def fib(n)
  return n if n < 2
  fib(n - 1) + fib(n - 2)
end

def main
  _t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)
  puts fib(34)
  $stderr.puts "bench_ns=#{Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond) - _t0}"
end

main
