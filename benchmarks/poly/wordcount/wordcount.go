package main

import (
	"fmt"
	"os"
	"sort"
	"strconv"
	"strings"
	"time"
)

func main() {
	t0 := time.Now()
	base := "the quick brown fox jumps over the lazy dog and runs away fast today"
	m := make(map[string]int)
	for r := 0; r < 120000; r++ {
		line := strconv.Itoa(r%1000) + " " + base
		for _, w := range strings.Split(line, " ") {
			m[w]++
		}
	}
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	acc := 0
	for _, k := range keys {
		acc = (acc*31 + m[k]) % 1000000007
	}
	fmt.Println(len(m))
	fmt.Println(acc)
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
