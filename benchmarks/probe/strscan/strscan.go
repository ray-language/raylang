package main

import (
	"fmt"
	"os"
	"time"
)

func main() {
	t0 := time.Now()
	line := "GET /api/v1/users/12345?limit=50&offset=200 HTTP/1.1 host=example.com ua=Mozilla/5.0 status=200 bytes=5123"
	digits, words, numsum := 0, 0, 0
	for r := 0; r < 40000; r++ {
		cur := 0; inNum := false
		for _, c := range line {
			if c >= '0' && c <= '9' { digits++; cur = cur*10 + int(c-'0'); inNum = true } else {
				if inNum { numsum = (numsum + cur) % 1000003; cur = 0; inNum = false }
				if c == ' ' { words++ }
			}
		}
		if inNum { numsum = (numsum + cur) % 1000003 }
	}
	fmt.Println(digits); fmt.Println(words); fmt.Println(numsum)
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
