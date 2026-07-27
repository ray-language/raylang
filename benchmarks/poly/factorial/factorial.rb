def fact(n)
  return 1 if n < 2
  n * fact(n - 1)
end

def main
  _t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)
  (0..9).each do |i|
    puts fact(i)
  end
  $stderr.puts "bench_ns=#{Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond) - _t0}"
end

main
