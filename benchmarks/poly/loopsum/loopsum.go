package main

import (
	"fmt"
	"os"
	"time"
)

func main() {
	t0 := time.Now()
	acc := 0
	for i := 1; i <= 10000000; i++ {
		acc = (acc + i*i) % 1000000007
	}
	fmt.Println(acc)
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
