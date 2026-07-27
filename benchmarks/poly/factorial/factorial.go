package main

import (
	"fmt"
	"os"
	"time"
)

func fact(n int) int {
	if n < 2 {
		return 1
	}
	return n * fact(n-1)
}

func main() {
	t0 := time.Now()
	for i := 0; i < 10; i++ {
		fmt.Println(fact(i))
	}
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
