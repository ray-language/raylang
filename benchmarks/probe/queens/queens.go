package main

import (
	"fmt"
	"os"
	"time"
)
func solve(row, n int, cols, d1, d2 []bool) int {
	if row == n { return 1 }
	count := 0
	for c := 0; c < n; c++ {
		a, b := row+c, row-c+n-1
		if !cols[c] && !d1[a] && !d2[b] {
			cols[c], d1[a], d2[b] = true, true, true
			count += solve(row+1, n, cols, d1, d2)
			cols[c], d1[a], d2[b] = false, false, false
		}
	}
	return count
}
func main() {
	t0 := time.Now()
	n := 11
	fmt.Println(solve(0, n, make([]bool, n), make([]bool, 2*n), make([]bool, 2*n)))
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
