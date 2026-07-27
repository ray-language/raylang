package main

import (
	"fmt"
	"os"
	"time"
)

type Node struct {
	left, right *Node
}

func makeTree(depth int) *Node {
	if depth == 0 {
		return nil
	}
	return &Node{left: makeTree(depth - 1), right: makeTree(depth - 1)}
}

func nodeCount(n *Node) int {
	if n == nil {
		return 0
	}
	return 1 + nodeCount(n.left) + nodeCount(n.right)
}

func main() {
	t0 := time.Now()
	minDepth := 4
	maxDepth := 14

	stretch := makeTree(maxDepth + 1)
	fmt.Println(nodeCount(stretch))

	longLived := makeTree(maxDepth)

	totalCheck := 0
	for depth := minDepth; depth <= maxDepth; depth += 2 {
		iterations := 1 << (maxDepth - depth + minDepth)
		check := 0
		for i := 0; i < iterations; i++ {
			check += nodeCount(makeTree(depth))
		}
		totalCheck += check
	}

	fmt.Println(nodeCount(longLived))
	fmt.Println(totalCheck)
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
