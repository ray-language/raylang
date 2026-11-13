package main

import (
	"fmt"
	"os"
	"time"
)
type P struct{ x, y, vx, vy float64 }

func main() {
	t0 := time.Now()
	ps := []*P{}
	for i := 0; i < 1000; i++ {
		f := float64(i)
		ps = append(ps, &P{f, f * 0.5, 1.0 + f*0.001, 0.5 - f*0.0005})
	}
	for step := 0; step < 3000; step++ {
		for _, p := range ps {
			p.x = p.x + p.vx*0.01
			p.y = p.y + p.vy*0.01
			if p.x > 1000.0 { p.vx = 0.0 - p.vx }
			if p.y < 0.0 { p.vy = 0.0 - p.vy }
		}
	}
	sum := 0.0
	for _, p := range ps { sum += p.x + p.y }
	fmt.Println(int(sum))
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
