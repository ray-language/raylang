import sys, time


class Node:
    __slots__ = ("left", "right")

    def __init__(self, left, right):
        self.left = left
        self.right = right


def make_tree(depth):
    if depth == 0:
        return None
    return Node(make_tree(depth - 1), make_tree(depth - 1))


def node_count(n):
    if n is None:
        return 0
    return 1 + node_count(n.left) + node_count(n.right)


def main():
    _t0 = time.perf_counter_ns()
    min_depth = 4
    max_depth = 14

    stretch = make_tree(max_depth + 1)
    print(node_count(stretch))

    long_lived = make_tree(max_depth)

    total_check = 0
    depth = min_depth
    while depth <= max_depth:
        iterations = 1 << (max_depth - depth + min_depth)
        check = 0
        for _ in range(iterations):
            check += node_count(make_tree(depth))
        total_check += check
        depth += 2

    print(node_count(long_lived))
    print(total_check)
    print(f"bench_ns={time.perf_counter_ns() - _t0}", file=sys.stderr)


if __name__ == "__main__":
    main()
