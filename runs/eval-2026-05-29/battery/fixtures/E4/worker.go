package main

import (
	"fmt"
	"sort"
)

func dedupeAndSort(in []int) []int {
	seen := make(map[int]struct{}, len(in))
	out := make([]int, 0, len(in))
	for _, v := range in {
		if _, ok := seen[v]; ok {
			continue
		}
		seen[v] = struct{}{}
		out = append(out, v)
	}
	sort.Ints(out)
	return out
}

func main() {
	fmt.Println(dedupeAndSort([]int{5, 3, 5, 1, 3, 9, 1}))
}
