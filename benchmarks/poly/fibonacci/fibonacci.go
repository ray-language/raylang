package main

import (
	"fmt"
	"os"
	"time"
)

func fib(n int) int {
	if n < 2 {
		return n
	}
	return fib(n-1) + fib(n-2)
}

func main() {
	t0 := time.Now()
	for i := 0; i < 10; i++ {
		fmt.Println(fib(i))
	}
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
