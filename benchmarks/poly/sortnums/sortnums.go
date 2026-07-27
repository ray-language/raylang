package main

import (
	"fmt"
	"os"
	"sort"
	"time"
)

func main() {
	t0 := time.Now()
	n := 1000000
	seed := 12345
	arr := make([]int, n)
	for i := 0; i < n; i++ {
		seed = (48271 * seed) % 2147483647
		arr[i] = seed % 1000000
	}

	sort.Ints(arr)

	checksum := 0
	for _, v := range arr {
		checksum = (checksum*31 + v) % 1000000007
	}

	fmt.Println(arr[0])
	fmt.Println(arr[n-1])
	fmt.Println(checksum)
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
