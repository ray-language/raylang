package main

import (
	"fmt"
	"os"
	"time"
)
type Shape interface{ area() int }
type Sq struct{ s int }
type Rc struct{ w, h int }
func (q Sq) area() int { return q.s * q.s }
func (r Rc) area() int { return r.w * r.h }
func total[T Shape](xs []T) int {
	t := 0
	for _, x := range xs { t = (t + x.area()) % 1000003 }
	return t
}
func totalDyn(xs []Shape) int {
	t := 0
	for _, x := range xs { t = (t + x.area()) % 1000003 }
	return t
}
func main() {
	t0 := time.Now()
	sq := []Sq{}; rc := []Rc{}; mixed := []Shape{}
	for i := 0; i < 10000; i++ {
		sq = append(sq, Sq{i % 100}); rc = append(rc, Rc{i % 50, i % 30})
		if i%2 == 0 { mixed = append(mixed, Sq{i % 100}) } else { mixed = append(mixed, Rc{i % 50, i % 30}) }
	}
	acc := 0
	for r := 0; r < 500; r++ { acc = (acc + total(sq) + total(rc) + totalDyn(mixed)) % 1000003 }
	fmt.Println(acc)
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
