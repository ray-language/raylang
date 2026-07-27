def main
  _t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)
  acc = 0
  (1..10000000).each do |i|
    acc = (acc + i * i) % 1000000007
  end
  puts acc
  $stderr.puts "bench_ns=#{Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond) - _t0}"
end

main
