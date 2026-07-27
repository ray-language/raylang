def main
  _t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)
  n = 200000
  pattern = /^user(\d+) GET \/api\/(\d+) (\d+) (\d+)ms$/
  checksum = 0
  match_count = 0

  n.times do |i|
    status = i % 5 != 4 ? 200 : 404
    line = "user#{i} GET /api/#{i % 50} #{status} #{i % 250}ms"

    m = pattern.match(line)
    if m
      match_count += 1
      uid, path, st, ms = m.captures.map(&:to_i)
      checksum = (checksum * 31 + uid + path + st + ms) % 1000000007
    end
  end

  puts match_count
  puts checksum
  $stderr.puts "bench_ns=#{Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond) - _t0}"
end

main
