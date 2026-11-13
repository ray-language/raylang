package main

import (
	"fmt"
	"os"
	"time"
)
type Op struct{ kind, v int }
const ( Push = iota; Add; Mul; Dup; Swap; Mod )

func main() {
	t0 := time.Now()
	prog := []Op{}
	for i := 0; i < 200; i++ {
		prog = append(prog, Op{Push, i + 1}, Op{Add, 0}, Op{Dup, 0}, Op{Mul, 0}, Op{Mod, 1000003})
	}
	total := 0
	for r := 0; r < 3000; r++ {
		stack := []int{r}
		for _, op := range prog {
			switch op.kind {
			case Push: stack = append(stack, op.v)
			case Add: b := stack[len(stack)-1]; a := stack[len(stack)-2]; stack = append(stack[:len(stack)-2], a+b)
			case Mul: b := stack[len(stack)-1]; a := stack[len(stack)-2]; stack = append(stack[:len(stack)-2], a*b)
			case Dup: stack = append(stack, stack[len(stack)-1])
			case Swap: n := len(stack); stack[n-1], stack[n-2] = stack[n-2], stack[n-1]
			case Mod: stack[len(stack)-1] = stack[len(stack)-1] % op.v
			}
		}
		total = (total + stack[0]) % 1000003
	}
	fmt.Println(total)
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
