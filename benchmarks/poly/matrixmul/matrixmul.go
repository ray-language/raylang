package main

import (
	"fmt"
	"os"
	"time"
)

func main() {
	t0 := time.Now()
	n := 200
	a := make([][]float64, n)
	b := make([][]float64, n)
	for i := 0; i < n; i++ {
		a[i] = make([]float64, n)
		b[i] = make([]float64, n)
		for j := 0; j < n; j++ {
			a[i][j] = float64((i*n + j) % 13)
			b[i][j] = float64((j*n + i) % 17)
		}
	}

	c := make([][]float64, n)
	for i := 0; i < n; i++ {
		c[i] = make([]float64, n)
		for j := 0; j < n; j++ {
			s := 0.0
			for k := 0; k < n; k++ {
				s += a[i][k] * b[k][j]
			}
			c[i][j] = s
		}
	}

	checksum := 0.0
	for i := 0; i < n; i++ {
		for j := 0; j < n; j++ {
			checksum += c[i][j]
		}
	}

	fmt.Println(int(checksum))
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
