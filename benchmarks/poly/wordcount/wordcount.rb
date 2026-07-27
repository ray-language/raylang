_t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)
base = "the quick brown fox jumps over the lazy dog and runs away fast today"
m = Hash.new(0)
120000.times do |r|
  line = (r % 1000).to_s + " " + base
  line.split(" ").each { |w| m[w] += 1 }
end
acc = 0
m.keys.sort.each { |k| acc = (acc * 31 + m[k]) % 1000000007 }
puts m.size
puts acc
$stderr.puts "bench_ns=#{Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond) - _t0}"
