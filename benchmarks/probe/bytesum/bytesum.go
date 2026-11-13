package main

import (
	"fmt"
	"os"
	"time"
	"strings"
)

func main() {
	t0 := time.Now()
	data := []byte(strings.Repeat("the quick brown fox jumps over the lazy dog 0123456789 ", 40000))
	a, b := 1, 0
	for r := 0; r < 4; r++ {
		for i := 0; i < len(data); i++ {
			a = (a + int(data[i])) % 65521
			b = (b + a) % 65521
		}
	}
	fmt.Println(b*65536 + a)
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
