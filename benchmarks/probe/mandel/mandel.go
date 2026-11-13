package main

import (
	"fmt"
	"os"
	"time"
)

func main() {
	t0 := time.Now()
	count := 0
	n := 500
	for py := 0; py < n; py++ {
		for px := 0; px < n; px++ {
			cr := float64(px)*3.0/float64(n) - 2.0
			ci := float64(py)*2.0/float64(n) - 1.0
			zr, zi := 0.0, 0.0
			it := 0
			for it < 200 && zr*zr+zi*zi < 4.0 {
				t := zr*zr - zi*zi + cr
				zi = 2.0*zr*zi + ci
				zr = t
				it++
			}
			count += it
		}
	}
	fmt.Println(count)
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
