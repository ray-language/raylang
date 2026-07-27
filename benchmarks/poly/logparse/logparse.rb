_t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)
statuses = ["200", "200", "200", "404", "500"]
cnt = Hash.new(0)
lat = Hash.new(0)
150000.times do |i|
  path = "/api/" + (i % 50).to_s
  status = statuses[i % 5]
  line = "GET " + path + " " + status + " " + (i % 250).to_s
  f = line.split(" ")
  cnt[f[2]] += 1
  lat[f[1]] += f[3].to_i
end
cnt.keys.sort.each { |k| puts k + " " + cnt[k].to_s }
puts lat.values.sum
$stderr.puts "bench_ns=#{Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond) - _t0}"
