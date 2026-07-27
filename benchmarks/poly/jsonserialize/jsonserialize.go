package main

import (
	"fmt"
	"os"
	"strconv"
	"strings"
	"time"
)

func main() {
	t0 := time.Now()
	parts := make([]string, 0, 400000)
	for i := 0; i < 400000; i++ {
		s := strconv.Itoa(i)
		parts = append(parts, `{"id":`+s+`,"name":"user`+s+`","score":`+strconv.Itoa(i%100)+`}`)
	}
	out := strings.Join(parts, "\n")
	fmt.Println(len(out))
	fmt.Println(len(parts))
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
