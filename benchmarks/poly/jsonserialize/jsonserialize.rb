_t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)
parts = []
400000.times do |i|
  parts << '{"id":' + i.to_s + ',"name":"user' + i.to_s + '","score":' + (i % 100).to_s + '}'
end
out = parts.join("\n")
puts out.length
puts parts.length
$stderr.puts "bench_ns=#{Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond) - _t0}"
