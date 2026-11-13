package main

import (
	"fmt"
	"os"
	"time"
)

func main() {
	t0 := time.Now()
	n := 3000000
	sieve := []bool{}
	for i := 0; i < n; i++ { sieve = append(sieve, true) }
	for i := 2; i*i < n; i++ {
		if sieve[i] { for j := i * i; j < n; j += i { sieve[j] = false } }
	}
	primes := 0
	for k := 2; k < n; k++ { if sieve[k] { primes++ } }
	fmt.Println(primes)
	w := 700
	g := [][]int{}
	for y := 0; y < w; y++ {
		row := []int{}
		for x := 0; x < w; x++ { row = append(row, (x*31+y*17)%10) }
		g = append(g, row)
	}
	for y := 1; y < w; y++ {
		for x := 1; x < w; x++ {
			up, left := g[y-1][x], g[y][x-1]
			best := left
			if up < left { best = up }
			g[y][x] = (g[y][x] + best) % 1000003
		}
	}
	fmt.Println(g[w-1][w-1])
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
