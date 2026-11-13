package main

import (
	"fmt"
	"os"
	"time"
	"strings"
)

func main() {
	t0 := time.Now()
	total := 0
	parts := []string{}
	for i := 0; i < 300000; i++ {
		s := fmt.Sprintf("id=%d name=user%d ok=%t", i, i%1000, i%2 == 0)
		total += len(s)
		if i%10 == 0 { parts = append(parts, s) }
	}
	joined := strings.Join(parts, ",")
	fmt.Println(total)
	fmt.Println(len(joined))
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
