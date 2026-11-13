package main

import (
	"fmt"
	"os"
	"time"
)

func mapf(xs []int, f func(int) int) []int {
	out := make([]int, 0, len(xs))
	for _, x := range xs { out = append(out, f(x)) }
	return out
}
func filterf(xs []int, f func(int) bool) []int {
	out := []int{}
	for _, x := range xs { if f(x) { out = append(out, x) } }
	return out
}
func foldf(xs []int, init int, f func(int, int) int) int {
	a := init
	for _, x := range xs { a = f(a, x) }
	return a
}
func main() {
	t0 := time.Now()
	xs := []int{}
	for i := 0; i < 100000; i++ { xs = append(xs, i) }
	acc := 0
	for r := 0; r < 100; r++ {
		s := foldf(filterf(mapf(xs, func(x int) int { return x*3 + r }), func(x int) bool { return x%2 == 0 }), 0, func(a, x int) int { return (a + x) % 1000003 })
		acc = (acc + s) % 1000003
	}
	fmt.Println(acc)
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
