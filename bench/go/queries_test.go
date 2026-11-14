package bench

import (
	"cmp"
	"slices"
	"strconv"
	"strings"
	"testing"
)

type order struct {
	id, customer, cents int
	region              string
}

type customer struct {
	id   int
	name string
}

// orders gives count orders from a linear congruential generator, as the
// Tondo side makes them.
func orders(count int) []order {
	regions := []string{"north", "south", "east", "west"}
	var all []order
	seed := 42
	for id := 0; id < count; id++ {
		seed = (seed*1_103_515_245 + 12345) % 2_147_483_648
		all = append(all, order{id: id, customer: seed % 1000, cents: seed % 100_000, region: regions[seed%4]})
	}
	return all
}

func customers(count int) []customer {
	var all []customer
	for id := 0; id < count; id++ {
		all = append(all, customer{id: id, name: "customer " + strconv.Itoa(id)})
	}
	return all
}

// The eager chains build each array, as Tondo's methods of Array do; the
// lazy ones are the loop Go writes for them.
func BenchmarkQueries(b *testing.B) {
	all := orders(10_000)
	amounts := make([]int, 0, len(all))
	for _, o := range all {
		amounts = append(amounts, o.cents)
	}
	b.Run("numbers map filter sum 10000", func(b *testing.B) {
		for b.Loop() {
			doubled := make([]int, 0, len(amounts))
			for _, n := range amounts {
				doubled = append(doubled, n*2)
			}
			var kept []int
			for _, n := range doubled {
				if n%3 == 0 {
					kept = append(kept, n)
				}
			}
			total := 0
			for _, n := range kept {
				total += n
			}
			sinkInt = total
		}
	})
	b.Run("numbers iter map filter sum 10000", func(b *testing.B) {
		for b.Loop() {
			total := 0
			for _, n := range amounts {
				if n*2%3 == 0 {
					total += n * 2
				}
			}
			sinkInt = total
		}
	})
	b.Run("filter sumBy 10000", func(b *testing.B) {
		for b.Loop() {
			var kept []order
			for _, o := range all {
				if o.cents > 50_000 {
					kept = append(kept, o)
				}
			}
			total := 0
			for _, o := range kept {
				total += o.cents
			}
			sinkInt = total
		}
	})
	b.Run("iter filter sumBy 10000", func(b *testing.B) {
		for b.Loop() {
			total := 0
			for _, o := range all {
				if o.cents > 50_000 {
					total += o.cents
				}
			}
			sinkInt = total
		}
	})
	b.Run("groupBy 10000", func(b *testing.B) {
		for b.Loop() {
			groups := map[int][]order{}
			for _, o := range all {
				groups[o.customer] = append(groups[o.customer], o)
			}
			sinkInt = len(groups)
		}
	})
	b.Run("sortedBy 10000", func(b *testing.B) {
		for b.Loop() {
			sorted := slices.Clone(all)
			slices.SortStableFunc(sorted, func(a, b order) int { return cmp.Compare(a.cents, b.cents) })
			sinkAny = sorted
		}
	})
	b.Run("countBy 10000", func(b *testing.B) {
		for b.Loop() {
			counts := map[string]int{}
			for _, o := range all {
				counts[o.region]++
			}
			sinkAny = counts
		}
	})
	b.Run("maxBy 10000", func(b *testing.B) {
		for b.Loop() {
			best := all[0]
			for _, o := range all[1:] {
				if o.cents > best.cents {
					best = o
				}
			}
			sinkInt = best.id
		}
	})
	b.Run("keyedBy 10000", func(b *testing.B) {
		for b.Loop() {
			keyed := map[int]order{}
			for _, o := range all {
				keyed[o.id] = o
			}
			sinkAny = keyed
		}
	})
	b.Run("uniqueBy 10000", func(b *testing.B) {
		for b.Loop() {
			seen := map[int]struct{}{}
			var unique []order
			for _, o := range all {
				if _, ok := seen[o.customer]; !ok {
					seen[o.customer] = struct{}{}
					unique = append(unique, o)
				}
			}
			sinkAny = unique
		}
	})
	b.Run("averageBy 10000", func(b *testing.B) {
		for b.Loop() {
			total := 0.0
			for _, o := range all {
				total += float64(o.cents)
			}
			sinkFloat = total / float64(len(all))
		}
	})
	b.Run("toSet 10000", func(b *testing.B) {
		for b.Loop() {
			set := map[int]struct{}{}
			for _, n := range amounts {
				set[n] = struct{}{}
			}
			sinkAny = set
		}
	})
	people := customers(1000)
	b.Run("joinBy 10000x1000", func(b *testing.B) {
		for b.Loop() {
			byID := map[int][]customer{}
			for _, p := range people {
				byID[p.id] = append(byID[p.id], p)
			}
			type pair struct {
				o order
				c customer
			}
			var pairs []pair
			for _, o := range all {
				for _, p := range byID[o.customer] {
					pairs = append(pairs, pair{o, p})
				}
			}
			sinkInt = len(pairs)
		}
	})
}

const (
	asciiWords = "the quick brown fox jumps over the lazy dog "
	otherWords = "el ñandú comió más piñones αβγ δέλτα 日本語の文章 "
)

func counted(text string) int {
	count := 0
	for range text {
		count++
	}
	return count
}

func BenchmarkTexts(b *testing.B) {
	ascii, other := strings.Repeat(asciiWords, 25), strings.Repeat(otherWords, 25)
	b.Run("replace ascii", func(b *testing.B) {
		for b.Loop() {
			sinkString = strings.ReplaceAll(ascii, "the", "a")
		}
	})
	b.Run("replace other", func(b *testing.B) {
		for b.Loop() {
			sinkString = strings.ReplaceAll(other, "más", "menos")
		}
	})
	padded := "   " + asciiWords + "\t\n"
	b.Run("trim", func(b *testing.B) {
		for b.Loop() {
			sinkString = strings.TrimSpace(padded)
		}
	})
	b.Run("toUpper ascii", func(b *testing.B) {
		for b.Loop() {
			sinkString = strings.ToUpper(ascii)
		}
	})
	b.Run("toUpper other", func(b *testing.B) {
		for b.Loop() {
			sinkString = strings.ToUpper(other)
		}
	})
	words := repeat(strings.Split(asciiWords, " "), 10)
	b.Run("join 100", func(b *testing.B) {
		for b.Loop() {
			sinkString = strings.Join(words, ", ")
		}
	})
	b.Run("split other", func(b *testing.B) {
		for b.Loop() {
			sinkInt = len(strings.Split(other, " "))
		}
	})
	b.Run("contains other", func(b *testing.B) {
		for b.Loop() {
			sinkBool = strings.Contains(other, "文章 x")
		}
	})
	written := make([]string, 1000)
	for n := range written {
		written[n] = strconv.Itoa(n * 7919)
	}
	b.Run("parse ints 1000", func(b *testing.B) {
		for b.Loop() {
			total := 0
			for _, text := range written {
				n, _ := strconv.Atoi(text)
				total += n
			}
			sinkInt = total
		}
	})
	b.Run("chars ascii", func(b *testing.B) {
		for b.Loop() {
			sinkInt = counted(ascii)
		}
	})
	b.Run("chars other", func(b *testing.B) {
		for b.Loop() {
			sinkInt = counted(other)
		}
	})
}
