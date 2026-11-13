package main

import (
	"fmt"
	"os"
	"time"
	"sort"
)
type Row struct{ id, score int; name string }

func main() {
	t0 := time.Now()
	rows := []*Row{}
	seed := 42
	for i := 0; i < 300000; i++ {
		seed = (seed*1103515245 + 12345) % 2147483648
		rows = append(rows, &Row{i, seed % 100000, fmt.Sprintf("n%d", seed%1000)})
	}
	byScore := append([]*Row{}, rows...)
	sort.Slice(byScore, func(i, j int) bool { a, b := byScore[i], byScore[j]; return a.score < b.score || (a.score == b.score && a.id < b.id) })
	byName := append([]*Row{}, rows...)
	sort.Slice(byName, func(i, j int) bool { a, b := byName[i], byName[j]; return a.name < b.name || (a.name == b.name && a.id < b.id) })
	fmt.Println(byScore[0].id + byScore[299999].id)
	fmt.Println(byName[0].id + byName[299999].id)
	fmt.Fprintf(os.Stderr, "bench_ns=%d\n", time.Since(t0).Nanoseconds())
}
