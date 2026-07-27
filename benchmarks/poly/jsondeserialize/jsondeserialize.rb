def main
  _t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)
  n = 400000
  checksum = 0
  total_name_len = 0

  n.times do |i|
    line = "{\"id\":#{i},\"name\":\"user#{i}\",\"score\":#{i % 100}}"

    id_val, name_val, score_val =
      line.match(/"id":(\d+),"name":"(user\d+)","score":(\d+)}/).captures

    checksum = (checksum * 31 + id_val.to_i + score_val.to_i) % 1000000007
    total_name_len += name_val.length
  end

  puts checksum
  puts total_name_len
  $stderr.puts "bench_ns=#{Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond) - _t0}"
end

main
