// The latency of the servers of BenchmarkHttp at a fixed rate of
// requests, as ../../latency/latency.to measures Tondo's: each request is
// sent at its time whether or not those before it came back, and its
// latency runs from that time until its answer. It writes a line of JSON
// for each server: {"name", "p50_us", "p99_us"}.
//
//	go run ./latency [-rate 20000] [-seconds 2]   (from bench/go)
package main

import (
	"bytes"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"slices"
	"sync"
	"time"
)

type payload struct {
	Name  string   `json:"name"`
	Count int      `json:"count"`
	Tags  []string `json:"tags"`
}

func routes() *http.ServeMux {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /hello", func(w http.ResponseWriter, _ *http.Request) {
		w.Write([]byte("hello"))
	})
	mux.HandleFunc("POST /json", func(w http.ResponseWriter, r *http.Request) {
		var input payload
		if err := json.NewDecoder(r.Body).Decode(&input); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		input.Count++
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(input)
	})
	return mux
}

// One request, sent when it is due: its latency, in microseconds.
func timed(client *http.Client, url string, due time.Time, asJSON bool) int64 {
	var response *http.Response
	var err error
	if asJSON {
		body, _ := json.Marshal(payload{Name: "item", Count: 1, Tags: []string{"a", "b"}})
		response, err = client.Post(url+"/json", "application/json", bytes.NewReader(body))
	} else {
		response, err = client.Get(url + "/hello")
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	if asJSON {
		var back payload
		json.NewDecoder(response.Body).Decode(&back)
	} else {
		io.ReadAll(response.Body)
	}
	response.Body.Close()
	return time.Since(due).Microseconds()
}

// rate requests a second for seconds, each in a goroutine of its own, after
// half a second that opens the connections and is not counted: the median
// latency and the 99th percentile, in microseconds.
func measured(client *http.Client, url string, rate, seconds int, asJSON bool) (int64, int64) {
	sent(client, url, rate, rate/2, asJSON)
	latencies := sent(client, url, rate, rate*seconds, asJSON)
	slices.Sort(latencies)
	return latencies[len(latencies)*50/100], latencies[len(latencies)*99/100]
}

// total requests at rate a second: the latency of each.
func sent(client *http.Client, url string, rate, total int, asJSON bool) []int64 {
	gap := time.Second / time.Duration(rate)
	latencies := make([]int64, total)
	var group sync.WaitGroup
	start := time.Now()
	for i := 0; i < total; i++ {
		due := start.Add(time.Duration(i) * gap)
		if early := time.Until(due); early > 0 {
			time.Sleep(early)
		}
		group.Add(1)
		go func(i int) {
			defer group.Done()
			latencies[i] = timed(client, url, due, asJSON)
		}(i)
	}
	group.Wait()
	return latencies
}

func written(name string, rate int, p50, p99 int64) {
	fmt.Printf("{\"name\":\"%s %d/s\",\"p50_us\":%d,\"p99_us\":%d}\n", name, rate, p50, p99)
}

func main() {
	rate := flag.Int("rate", 20_000, "requests a second")
	seconds := flag.Int("seconds", 2, "seconds to send them for")
	flag.Parse()
	plain := httptest.NewServer(routes())
	client := &http.Client{Transport: &http.Transport{MaxIdleConnsPerHost: 64}}
	p50, p99 := measured(client, plain.URL, *rate, *seconds, false)
	written("http/hello", *rate, p50, p99)
	p50, p99 = measured(client, plain.URL, *rate, *seconds, true)
	written("http/json", *rate, p50, p99)
	plain.Close()
	read := func(path string) []byte {
		text, err := os.ReadFile(path)
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
		return text
	}
	certificate, err := tls.X509KeyPair(read("../tondo/src/tls/cert.pem"), read("../tondo/src/tls/key.pem"))
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	authority := x509.NewCertPool()
	authority.AppendCertsFromPEM(read("../tondo/src/tls/ca.pem"))
	secure := httptest.NewUnstartedServer(routes())
	secure.EnableHTTP2 = true
	secure.TLS = &tls.Config{Certificates: []tls.Certificate{certificate}}
	secure.StartTLS()
	trusting := &http.Client{Transport: &http.Transport{
		MaxIdleConnsPerHost: 64,
		ForceAttemptHTTP2:   true,
		TLSClientConfig:     &tls.Config{RootCAs: authority},
	}}
	p50, p99 = measured(trusting, secure.URL, *rate, *seconds, false)
	written("http/tls hello", *rate, p50, p99)
	secure.Close()
}
