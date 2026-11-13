package main

import (
	"fmt"
	"os"
	"time"
	"sync"
)

func main() {
	t0 := time.Now()
	ch := make(chan int, 64)
	n := 1000000
	go func() { for i := 0; i < n; i++ { ch <- i }; close(ch) }()
	sum := 0
	for v := range ch { sum = (sum + v) % 1000003 }
	fmt.Println(sum)
	acc := 0
	for r := 0; r < 100; r++ {
		res := make([]int, 1000)
		var wg sync.WaitGroup
		for i := 0; i < 1000; i++ { wg.Add(1); go func(i int) { res[i] = i*2 + r; wg.Done() }(i) }
		wg.Wait()
		for _, v := range res { acc = (acc + v) % 1000003 }
	}
	fmt.Println(acc)
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
