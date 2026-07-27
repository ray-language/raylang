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
	statuses := []string{"200", "200", "200", "404", "500"}
	cnt := make(map[string]int)
	lat := make(map[string]int)
	for i := 0; i < 150000; i++ {
		path := "/api/" + strconv.Itoa(i%50)
		status := statuses[i%5]
		line := "GET " + path + " " + status + " " + strconv.Itoa(i%250)
		f := strings.Split(line, " ")
		cnt[f[2]]++
		n, _ := strconv.Atoi(f[3])
		lat[f[1]] += n
	}
	keys := make([]string, 0, len(cnt))
	for k := range cnt {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	for _, k := range keys {
		fmt.Println(k + " " + strconv.Itoa(cnt[k]))
	}
	total := 0
	for _, v := range lat {
		total += v
	}
	fmt.Println(total)
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
