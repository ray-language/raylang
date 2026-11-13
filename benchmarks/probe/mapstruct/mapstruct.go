package main

import (
	"fmt"
	"os"
	"time"
)
type Acc struct{ balance, n int }

func main() {
	t0 := time.Now()
	m := map[int]*Acc{}
	for i := 0; i < 100000; i++ { m[i] = &Acc{} }
	seed := 12345
	for i := 0; i < 2000000; i++ {
		seed = (seed*1103515245 + 12345) % 2147483648
		k := seed % 100000
		if a, ok := m[k]; ok { a.balance += seed % 100; a.n++ }
	}
	sum := 0
	for i := 0; i < 100000; i++ { if a, ok := m[i]; ok { sum = (sum + a.balance*31 + a.n) % 1000000007 } }
	fmt.Println(sum)
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
