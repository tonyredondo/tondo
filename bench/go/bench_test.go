// The Go side of the comparison with Tondo: each benchmark does the same
// work as the Tondo benchmark of the same name in ../tondo, and `b.Loop`
// keeps the compiler from dropping it, as `testing.bench` does.
package bench

import (
	"encoding/json"
	"fmt"
	"math"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"sync"
	"testing"
)

// What the benchmarks give, kept so that the compiler keeps the work; of
// their own types, so that keeping them allocates nothing.
var (
	sinkInt    int
	sinkFloat  float64
	sinkBool   bool
	sinkString string
	sinkInts   []int
	sinkAny    any
	sinkItem   item
)

func fib(n int) int {
	if n < 2 {
		return n
	}
	return fib(n-1) + fib(n-2)
}

func apply(f func(int) int, times int) int {
	total := 0
	for i := 0; i < times; i++ {
		total += f(i)
	}
	return total
}

func BenchmarkCalls(b *testing.B) {
	b.Run("fib 25", func(b *testing.B) {
		for b.Loop() {
			sinkInt = fib(25)
		}
	})
	step := 3
	b.Run("closure 1000", func(b *testing.B) {
		for b.Loop() {
			sinkInt = apply(func(x int) int { return x + step }, 1000)
		}
	})
}

func sum(count int) int {
	total := 0
	for i := 0; i < count; i++ {
		total += i * i % 7
	}
	return total
}

func collatz(limit int) int {
	steps := 0
	for start := 1; start < limit; start++ {
		n := start
		for n != 1 {
			if n%2 == 0 {
				n = n / 2
			} else {
				n = 3*n + 1
			}
			steps++
		}
	}
	return steps
}

func BenchmarkLoops(b *testing.B) {
	b.Run("sum 1000000", func(b *testing.B) {
		for b.Loop() {
			sinkInt = sum(1_000_000)
		}
	})
	b.Run("collatz 10000", func(b *testing.B) {
		for b.Loop() {
			sinkInt = collatz(10_000)
		}
	})
}

func mandelbrot(size int) int {
	inside := 0
	for row := 0; row < size; row++ {
		for column := 0; column < size; column++ {
			cr := float64(column)*3.0/float64(size) - 2.0
			ci := float64(row)*2.0/float64(size) - 1.0
			zr, zi := 0.0, 0.0
			iteration := 0
			for iteration < 50 && zr*zr+zi*zi <= 4.0 {
				next := zr*zr - zi*zi + cr
				zi = 2.0*zr*zi + ci
				zr = next
				iteration++
			}
			if iteration == 50 {
				inside++
			}
		}
	}
	return inside
}

func roots(count int) float64 {
	total := 0.0
	for i := 0; i < count; i++ {
		total += math.Sqrt(float64(i))
	}
	return total
}

func BenchmarkFloats(b *testing.B) {
	b.Run("mandelbrot 64", func(b *testing.B) {
		for b.Loop() {
			sinkInt = mandelbrot(64)
		}
	})
	b.Run("sqrt 100000", func(b *testing.B) {
		for b.Loop() {
			sinkFloat = roots(100_000)
		}
	})
}

func joined(count int) string {
	var builder strings.Builder
	for i := 0; i < count; i++ {
		builder.WriteString("part")
		builder.WriteString(strconv.Itoa(i))
	}
	return builder.String()
}

func ints(count int) int {
	total := 0
	for i := 0; i < count; i++ {
		total += len(strconv.Itoa(i))
	}
	return total
}

func BenchmarkStrings(b *testing.B) {
	name, place := "ada", "london"
	b.Run("interpolate", func(b *testing.B) {
		for b.Loop() {
			sinkString = name + " lives in " + place + ", age " + strconv.Itoa(36)
		}
	})
	b.Run("builder 100", func(b *testing.B) {
		for b.Loop() {
			sinkString = joined(100)
		}
	})
	csv := strings.Join(repeat([]string{"alpha", "beta", "gamma", "delta"}, 25), ",")
	b.Run("split 100", func(b *testing.B) {
		for b.Loop() {
			sinkInt = len(strings.Split(csv, ","))
		}
	})
	b.Run("int to text 1000", func(b *testing.B) {
		for b.Loop() {
			sinkInt = ints(1000)
		}
	})
	haystack := strings.Repeat("abcdefghij", 100) + "needle"
	b.Run("contains 1000", func(b *testing.B) {
		for b.Loop() {
			sinkBool = strings.Contains(haystack, "needle")
		}
	})
}

func repeat(values []string, times int) []string {
	var out []string
	for i := 0; i < times; i++ {
		out = append(out, values...)
	}
	return out
}

// shuffled gives count numbers in a fixed shuffled order, from a linear
// congruential generator, as the Tondo side makes them.
func shuffled(count int) []int {
	var values []int
	seed := 42
	for i := 0; i < count; i++ {
		seed = (seed*1_103_515_245 + 12345) % 2_147_483_648
		values = append(values, seed%100_000)
	}
	return values
}

func pushed(count int) int {
	var values []int
	for i := 0; i < count; i++ {
		values = append(values, i)
	}
	return len(values)
}

func indexed(values []int) int {
	total := 0
	for i := 0; i < len(values); i++ {
		total += values[i]
	}
	return total
}

func BenchmarkArrays(b *testing.B) {
	b.Run("push 1000", func(b *testing.B) {
		for b.Loop() {
			sinkInt = pushed(1000)
		}
	})
	values := shuffled(10_000)
	b.Run("sort 10000", func(b *testing.B) {
		for b.Loop() {
			sorted := append([]int(nil), values...)
			sort.Ints(sorted)
			sinkInts = sorted
		}
	})
	b.Run("map filter sum 10000", func(b *testing.B) {
		for b.Loop() {
			doubled := make([]int, 0, len(values))
			for _, x := range values {
				doubled = append(doubled, x*2)
			}
			var kept []int
			for _, x := range doubled {
				if x%3 == 0 {
					kept = append(kept, x)
				}
			}
			total := 0
			for _, x := range kept {
				total += x
			}
			sinkInt = total
		}
	})
	b.Run("index sum 10000", func(b *testing.B) {
		for b.Loop() {
			sinkInt = indexed(values)
		}
	})
}

func inserted(count int) int {
	squares := map[int]int{}
	for i := 0; i < count; i++ {
		squares[i] = i * 2
	}
	return len(squares)
}

func looked(squares map[int]int, count int) int {
	total := 0
	for i := 0; i < count; i++ {
		total += squares[i]
	}
	return total
}

func named(count int) int {
	ages := map[string]int{}
	for i := 0; i < count; i++ {
		ages["key "+strconv.Itoa(i)] = i
	}
	return len(ages)
}

func BenchmarkMaps(b *testing.B) {
	b.Run("insert 10000 ints", func(b *testing.B) {
		for b.Loop() {
			sinkInt = inserted(10_000)
		}
	})
	squares := map[int]int{}
	for i := 0; i < 10_000; i++ {
		squares[i] = i * 2
	}
	b.Run("lookup 10000 ints", func(b *testing.B) {
		for b.Loop() {
			sinkInt = looked(squares, 10_000)
		}
	})
	b.Run("insert 1000 strings", func(b *testing.B) {
		for b.Loop() {
			sinkInt = named(1000)
		}
	})
}

type point struct{ x, y int }

type user struct {
	name string
	age  int
}

func points(count int) int {
	var all []point
	for i := 0; i < count; i++ {
		all = append(all, point{x: i, y: i * 2})
	}
	total := 0
	for _, p := range all {
		total += p.x + p.y
	}
	return total
}

func users(count int) int {
	var all []user
	for i := 0; i < count; i++ {
		all = append(all, user{name: "user " + strconv.Itoa(i), age: i % 90})
	}
	return len(all)
}

func BenchmarkStructs(b *testing.B) {
	b.Run("points 1000", func(b *testing.B) {
		for b.Loop() {
			sinkInt = points(1000)
		}
	})
	b.Run("users 1000", func(b *testing.B) {
		for b.Loop() {
			sinkInt = users(1000)
		}
	})
}

type item struct {
	ID        int      `json:"id"`
	Name      string   `json:"name"`
	Price     float64  `json:"price"`
	Tags      []string `json:"tags"`
	Available bool     `json:"available"`
}

const document = `{"id": 7, "name": "lamp", "price": 19.5, "tags": ["home", "light"], "available": true, "stock": 120, "color": "white", "weight": 1.25, "brand": "lumen", "code": "LMP-7"}`

func numbers(count int) string {
	parts := make([]string, 0, count)
	for i := 0; i < count; i++ {
		parts = append(parts, strconv.Itoa(i))
	}
	return "[" + strings.Join(parts, ",") + "]"
}

func BenchmarkJson(b *testing.B) {
	b.Run("parse object", func(b *testing.B) {
		for b.Loop() {
			var value any
			if err := json.Unmarshal([]byte(document), &value); err != nil {
				b.Fatal(err)
			}
			sinkAny = value
		}
	})
	array := numbers(1000)
	b.Run("parse 1000 numbers", func(b *testing.B) {
		for b.Loop() {
			var value any
			if err := json.Unmarshal([]byte(array), &value); err != nil {
				b.Fatal(err)
			}
			sinkAny = value
		}
	})
	b.Run("decode struct", func(b *testing.B) {
		for b.Loop() {
			var value item
			if err := json.Unmarshal([]byte(document), &value); err != nil {
				b.Fatal(err)
			}
			sinkItem = value
		}
	})
	var items []item
	for i := 0; i < 100; i++ {
		items = append(items, item{ID: i, Name: fmt.Sprintf("item %d", i), Price: float64(i) * 1.5, Tags: []string{"a", "b"}, Available: i%2 == 0})
	}
	b.Run("encode 100 structs", func(b *testing.B) {
		for b.Loop() {
			text, err := json.Marshal(items)
			if err != nil {
				b.Fatal(err)
			}
			sinkString = string(text)
		}
	})
}

func BenchmarkRegex(b *testing.B) {
	pattern := regexp.MustCompile(`^[a-z]+\.[a-z]+@[a-z]+\.com$`)
	lines := make([]string, 0, 1000)
	for i := 0; i < 1000; i++ {
		if i%2 == 0 {
			lines = append(lines, "ada.lovelace@example.com")
		} else {
			lines = append(lines, "not an address "+strconv.Itoa(i))
		}
	}
	b.Run("match 1000", func(b *testing.B) {
		for b.Loop() {
			found := 0
			for _, line := range lines {
				if pattern.MatchString(line) {
					found++
				}
			}
			sinkInt = found
		}
	})
}

func spawned(count int) int {
	results := make([]int, count)
	var group sync.WaitGroup
	for i := 0; i < count; i++ {
		group.Add(1)
		go func(i int) {
			defer group.Done()
			results[i] = i * i
		}(i)
	}
	group.Wait()
	total := 0
	for _, r := range results {
		total += r
	}
	return total
}

func piped(count int) int {
	values := make(chan int, 64)
	go func() {
		for i := 0; i < count; i++ {
			values <- i
		}
		close(values)
	}()
	total := 0
	for value := range values {
		total += value
	}
	return total
}

func BenchmarkTasks(b *testing.B) {
	b.Run("spawn 1000", func(b *testing.B) {
		for b.Loop() {
			sinkInt = spawned(1000)
		}
	})
	b.Run("channel 10000", func(b *testing.B) {
		for b.Loop() {
			sinkInt = piped(10_000)
		}
	})
}
