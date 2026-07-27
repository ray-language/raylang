// Carga `plaintext` — Go `net/http` de la librería estándar, sin router de terceros.
//
// Es el rival directo del banco: PERFORMANCE.md fija explícitamente la liga Node/Go. Sin
// GOMAXPROCS tocado (el default es todos los cores) y sin tuning: la comparación es "lo que
// da la stdlib tal cual", que es lo que usaría alguien que escribe un servicio en Go.
package main

import (
	"fmt"
	"net/http"
	"os"
)

func main() {
	if len(os.Args) < 3 {
		fmt.Fprintln(os.Stderr, "uso: plaintext-go <host> <puerto>")
		os.Exit(2)
	}
	addr := os.Args[1] + ":" + os.Args[2]

	http.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain")
		w.Write([]byte("Hello, World!"))
	})

	if err := http.ListenAndServe(addr, nil); err != nil {
		fmt.Fprintln(os.Stderr, "listen:", err)
		os.Exit(1)
	}
}
