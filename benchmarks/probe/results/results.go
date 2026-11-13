package main

import (
	"fmt"
	"os"
	"time"
	"errors"
)
var errBad = errors.New("bad")
func step(x int) (int, error) {
	if x%97 == 0 { return 0, errBad }
	return x * 2 % 1000003, nil
}
func run(x int) (int, error) {
	a, err := step(x)
	if err != nil { return 0, err }
	b, err := step(a + 1)
	if err != nil { return 0, err }
	return a + b, nil
}
func main() {
	t0 := time.Now()
	ok, sum := 0, 0
	for i := 0; i < 5000000; i++ {
		if v, err := run(i); err == nil { ok++; sum = (sum + v) % 1000003 }
	}
	fmt.Println(ok)
	fmt.Println(sum)
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
