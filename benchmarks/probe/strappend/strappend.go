package main

import (
	"fmt"
	"os"
	"time"
	"strconv"
)

func main() {
	t0 := time.Now()
	s := ""
	for i := 0; i < 60000; i++ { s = s + "ab" }
	fmt.Println(len(s))
	t := ""
	for i := 0; i < 200000; i++ { t = t + strconv.Itoa(i%10); if len(t) > 64 { t = "" } }
	fmt.Println(len(t))
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
