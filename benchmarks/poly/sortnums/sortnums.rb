def main
  _t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)
  n = 1000000
  seed = 12345
  arr = Array.new(n)
  n.times do |i|
    seed = (48271 * seed) % 2147483647
    arr[i] = seed % 1000000
  end

  arr.sort!

  checksum = 0
  arr.each do |v|
    checksum = (checksum * 31 + v) % 1000000007
  end

  puts arr[0]
  puts arr[n - 1]
  puts checksum
  $stderr.puts "bench_ns=#{Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond) - _t0}"
end

main
