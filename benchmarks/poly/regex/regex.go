package main

import (
	"fmt"
	"os"
	"regexp"
	"strconv"
	"time"
)

func main() {
	t0 := time.Now()
	n := 200000
	pattern := regexp.MustCompile(`^user(\d+) GET /api/(\d+) (\d+) (\d+)ms$`)
	checksum := 0
	matchCount := 0

	for i := 0; i < n; i++ {
		status := 200
		if i%5 == 4 {
			status = 404
		}
		line := fmt.Sprintf("user%d GET /api/%d %d %dms", i, i%50, status, i%250)

		m := pattern.FindStringSubmatch(line)
		if m != nil {
			matchCount++
			uid, _ := strconv.Atoi(m[1])
			path, _ := strconv.Atoi(m[2])
			st, _ := strconv.Atoi(m[3])
			ms, _ := strconv.Atoi(m[4])
			checksum = (checksum*31 + uid + path + st + ms) % 1000000007
		}
	}

	fmt.Println(matchCount)
	fmt.Println(checksum)
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
