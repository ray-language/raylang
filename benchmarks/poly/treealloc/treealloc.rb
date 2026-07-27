Node = Struct.new(:left, :right)

def make_tree(depth)
  return nil if depth == 0
  Node.new(make_tree(depth - 1), make_tree(depth - 1))
end

def node_count(n)
  return 0 if n.nil?
  1 + node_count(n.left) + node_count(n.right)
end

def main
  _t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)
  min_depth = 4
  max_depth = 14

  stretch = make_tree(max_depth + 1)
  puts node_count(stretch)

  long_lived = make_tree(max_depth)

  total_check = 0
  depth = min_depth
  while depth <= max_depth
    iterations = 1 << (max_depth - depth + min_depth)
    check = 0
    iterations.times do
      check += node_count(make_tree(depth))
    end
    total_check += check
    depth += 2
  end

  puts node_count(long_lived)
  puts total_check
  $stderr.puts "bench_ns=#{Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond) - _t0}"
end

main
