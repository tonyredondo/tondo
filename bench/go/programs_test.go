// The Go side of ../tondo/src/programs_test.to: small programs, each a
// shape programs have, written as a Go programmer would write them.
package bench

import (
	"bufio"
	"encoding/json"
	"sort"
	"strconv"
	"strings"
	"testing"
)

var sinkCounts map[string]int

// The words of a text, counted into a map.
func wordCounts(text string) map[string]int {
	counts := map[string]int{}
	for _, word := range strings.Split(text, " ") {
		if word == "" {
			continue
		}
		counts[word]++
	}
	return counts
}

// The numbers of a text, one a line, read and summed.
func lineSum(text string) (int, error) {
	scanner := bufio.NewScanner(strings.NewReader(text))
	total := 0
	for scanner.Scan() {
		number, err := strconv.Atoi(scanner.Text())
		if err != nil {
			number = 0
		}
		total += number
	}
	return total, scanner.Err()
}

type person struct {
	name string
	age  int
}

// The adults, by name, as a line of text.
func adultLine(people []person) string {
	var adults []person
	for _, p := range people {
		if p.age >= 18 {
			adults = append(adults, p)
		}
	}
	sort.SliceStable(adults, func(i, j int) bool { return adults[i].name < adults[j].name })
	parts := make([]string, 0, len(adults))
	for _, p := range adults {
		parts = append(parts, p.name+":"+strconv.Itoa(p.age))
	}
	return strings.Join(parts, ",")
}

type purchase struct {
	ID       int      `json:"id"`
	Customer string   `json:"customer"`
	Items    []string `json:"items"`
	Total    float64  `json:"total"`
	Paid     bool     `json:"paid"`
}

// An order read from JSON, changed, and written again.
func repriced(document string) string {
	var order purchase
	if err := json.Unmarshal([]byte(document), &order); err != nil {
		order = purchase{}
	}
	order.Total = order.Total * 1.1
	order.Paid = true
	text, err := json.Marshal(order)
	if err != nil {
		return ""
	}
	return string(text)
}

type node struct {
	value    int
	children []node
}

// A tree of that depth, each node with `fan` children.
func build(depth, fan int) node {
	var children []node
	if depth > 0 {
		for i := 0; i < fan; i++ {
			children = append(children, build(depth-1, fan))
		}
	}
	return node{value: depth, children: children}
}

func treeSum(n node) int {
	total := n.value
	for _, child := range n.children {
		total += treeSum(child)
	}
	return total
}

func BenchmarkPrograms(b *testing.B) {
	text := strings.Repeat("to be or not to be ", 100)
	b.Run("count words", func(b *testing.B) {
		for b.Loop() {
			sinkCounts = wordCounts(text)
		}
	})
	lines := make([]string, 0, 1000)
	for i := 0; i < 1000; i++ {
		lines = append(lines, strconv.Itoa(i*7))
	}
	column := strings.Join(lines, "\n")
	b.Run("sum lines", func(b *testing.B) {
		for b.Loop() {
			sinkInt, _ = lineSum(column)
		}
	})
	var people []person
	for i := 0; i < 1000; i++ {
		people = append(people, person{name: "person " + strconv.Itoa((i*7919)%1000), age: i % 60})
	}
	b.Run("sort adults", func(b *testing.B) {
		for b.Loop() {
			sinkString = adultLine(people)
		}
	})
	document := `{"id": 42, "customer": "ada", "items": ["lamp", "desk", "chair"], "total": 199.5, "paid": false}`
	b.Run("reprice order", func(b *testing.B) {
		for b.Loop() {
			sinkString = repriced(document)
		}
	})
	b.Run("build tree", func(b *testing.B) {
		for b.Loop() {
			sinkInt = treeSum(build(6, 4))
		}
	})
}
