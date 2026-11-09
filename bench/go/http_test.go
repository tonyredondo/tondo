// The Go side of ../tondo/src/http_test.to: a server and its clients in
// one process, over HTTP/1.1 in the clear and HTTP/2 over TLS, as Tondo's
// negotiate, with the same certificate; and lines echoed over TCP and TLS.
package bench

import (
	"bufio"
	"bytes"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"sync"
	"sync/atomic"
	"testing"
)

type payload struct {
	Name  string   `json:"name"`
	Count int      `json:"count"`
	Tags  []string `json:"tags"`
}

func routes() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /hello", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain; charset=utf-8")
		io.WriteString(w, "hello")
	})
	mux.HandleFunc("POST /json", func(w http.ResponseWriter, r *http.Request) {
		var p payload
		if err := json.NewDecoder(r.Body).Decode(&p); err != nil {
			http.Error(w, err.Error(), http.StatusBadRequest)
			return
		}
		p.Count++
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(p)
	})
	return mux
}

// The certificate of 127.0.0.1 and the authority that signed it.
func certificates(b *testing.B) (tls.Certificate, *x509.CertPool) {
	certificate, err := tls.LoadX509KeyPair("../tondo/src/tls/cert.pem", "../tondo/src/tls/key.pem")
	if err != nil {
		b.Fatal(err)
	}
	authority, err := os.ReadFile("../tondo/src/tls/ca.pem")
	if err != nil {
		b.Fatal(err)
	}
	pool := x509.NewCertPool()
	pool.AppendCertsFromPEM(authority)
	return certificate, pool
}

func hellos(client *http.Client, url string, count int) int {
	length := 0
	for i := 0; i < count; i++ {
		response, err := client.Get(url + "/hello")
		if err != nil {
			panic(err)
		}
		body, _ := io.ReadAll(response.Body)
		response.Body.Close()
		length += len(body)
	}
	return length
}

func posts(client *http.Client, url string, count int) int {
	total := 0
	for i := 0; i < count; i++ {
		sent, _ := json.Marshal(payload{Name: fmt.Sprintf("item %d", i), Count: i, Tags: []string{"a", "b"}})
		response, err := client.Post(url+"/json", "application/json", bytes.NewReader(sent))
		if err != nil {
			panic(err)
		}
		var back payload
		json.NewDecoder(response.Body).Decode(&back)
		response.Body.Close()
		total += back.Count
	}
	return total
}

func together(tasks, count int, each func(int) int) int {
	var total atomic.Int64
	var group sync.WaitGroup
	for i := 0; i < tasks; i++ {
		group.Add(1)
		go func() {
			defer group.Done()
			total.Add(int64(each(count)))
		}()
	}
	group.Wait()
	return int(total.Load())
}

func BenchmarkHttp(b *testing.B) {
	plain := httptest.NewServer(routes())
	defer plain.Close()
	client := &http.Client{Transport: &http.Transport{MaxIdleConnsPerHost: 64}}
	b.Run("hello 1x1000", func(b *testing.B) {
		for b.Loop() {
			sinkInt = hellos(client, plain.URL, 1000)
		}
	})
	b.Run("hello 64x100", func(b *testing.B) {
		for b.Loop() {
			sinkInt = together(64, 100, func(n int) int { return hellos(client, plain.URL, n) })
		}
	})
	b.Run("json 64x100", func(b *testing.B) {
		for b.Loop() {
			sinkInt = together(64, 100, func(n int) int { return posts(client, plain.URL, n) })
		}
	})
	certificate, authority := certificates(b)
	secure := httptest.NewUnstartedServer(routes())
	secure.EnableHTTP2 = true
	secure.TLS = &tls.Config{Certificates: []tls.Certificate{certificate}}
	secure.StartTLS()
	defer secure.Close()
	trusting := &http.Client{Transport: &http.Transport{
		MaxIdleConnsPerHost: 64,
		ForceAttemptHTTP2:   true,
		TLSClientConfig:     &tls.Config{RootCAs: authority},
	}}
	b.Run("tls 64x100", func(b *testing.B) {
		for b.Loop() {
			sinkInt = together(64, 100, func(n int) int { return hellos(trusting, secure.URL, n) })
		}
	})
}

// Sends back each line of every connection until it closes.
func echoing(listener net.Listener) {
	for {
		connection, err := listener.Accept()
		if err != nil {
			return
		}
		go func() {
			defer connection.Close()
			lines := bufio.NewReader(connection)
			for {
				line, err := lines.ReadString('\n')
				if err != nil {
					return
				}
				if _, err := io.WriteString(connection, line); err != nil {
					return
				}
			}
		}()
	}
}

func roundTrips(connection net.Conn, lines *bufio.Reader, count int) int {
	total := 0
	for i := 0; i < count; i++ {
		fmt.Fprintf(connection, "ping %d\n", i)
		line, err := lines.ReadString('\n')
		if err != nil {
			panic(err)
		}
		total += len(line) - 1
	}
	return total
}

func BenchmarkNet(b *testing.B) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		b.Fatal(err)
	}
	defer listener.Close()
	go echoing(listener)
	connection, err := net.Dial("tcp", listener.Addr().String())
	if err != nil {
		b.Fatal(err)
	}
	defer connection.Close()
	lines := bufio.NewReader(connection)
	b.Run("echo 1x1000", func(b *testing.B) {
		for b.Loop() {
			sinkInt = roundTrips(connection, lines, 1000)
		}
	})
	certificate, authority := certificates(b)
	secureListener, err := tls.Listen("tcp", "127.0.0.1:0", &tls.Config{Certificates: []tls.Certificate{certificate}})
	if err != nil {
		b.Fatal(err)
	}
	defer secureListener.Close()
	go echoing(secureListener)
	secure, err := tls.Dial("tcp", secureListener.Addr().String(), &tls.Config{RootCAs: authority})
	if err != nil {
		b.Fatal(err)
	}
	defer secure.Close()
	secureLines := bufio.NewReader(secure)
	b.Run("tls echo 1x1000", func(b *testing.B) {
		for b.Loop() {
			sinkInt = roundTrips(secure, secureLines, 1000)
		}
	})
}
