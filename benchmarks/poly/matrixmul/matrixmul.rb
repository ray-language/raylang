def main
  _t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)
  n = 200
  a = Array.new(n) { |i| Array.new(n) { |j| ((i * n + j) % 13).to_f } }
  b = Array.new(n) { |i| Array.new(n) { |j| ((j * n + i) % 17).to_f } }
  c = Array.new(n) { Array.new(n, 0.0) }

  n.times do |i|
    n.times do |j|
      s = 0.0
      n.times do |k|
        s += a[i][k] * b[k][j]
      end
      c[i][j] = s
    end
  end

  checksum = 0.0
  n.times do |i|
    n.times do |j|
      checksum += c[i][j]
    end
  end

  puts checksum.to_i
  $stderr.puts "bench_ns=#{Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond) - _t0}"
end

main
