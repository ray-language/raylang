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
	n := 400000
	checksum := 0
	totalNameLen := 0

	for i := 0; i < n; i++ {
		line := `{"id":` + strconv.Itoa(i) + `,"name":"user` + strconv.Itoa(i) + `","score":` + strconv.Itoa(i%100) + `}`

		idStart := strings.Index(line, ":") + 1
		idEnd := strings.Index(line, ",")
		idVal, _ := strconv.Atoi(line[idStart:idEnd])

		namePrefix := `"name":"`
		nameStart := strings.Index(line, namePrefix) + len(namePrefix)
		nameEnd := strings.Index(line[nameStart:], `"`) + nameStart
		nameVal := line[nameStart:nameEnd]

		scoreStart := strings.LastIndex(line, ":") + 1
		scoreEnd := strings.LastIndex(line, "}")
		scoreVal, _ := strconv.Atoi(line[scoreStart:scoreEnd])

		checksum = (checksum*31 + idVal + scoreVal) % 1000000007
		totalNameLen += len(nameVal)
	}

	fmt.Println(checksum)
	fmt.Println(totalNameLen)
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
