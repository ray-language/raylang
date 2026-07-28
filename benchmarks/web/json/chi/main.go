// Carga `json` — escalón de FRAMEWORK: Go con el router chi (el minimalista idiomático).
//
// Mismas 10 rutas y misma respuesta que las otras tres implementaciones: el `id` se interpola
// como string, sin parsear, para que ninguna pague un trabajo que otra no hace.
package main

import (
	"fmt"
	"net/http"
	"os"

	"github.com/go-chi/chi/v5"
)

func main() {
	if len(os.Args) < 3 {
		fmt.Fprintln(os.Stderr, "uso: json-chi <host> <puerto>")
		os.Exit(2)
	}
	addr := os.Args[1] + ":" + os.Args[2]

	r := chi.NewRouter()
	r.Get("/users/{id}", func(w http.ResponseWriter, req *http.Request) {
		id := chi.URLParam(req, "id")
		w.Header().Set("Content-Type", "application/json")
		fmt.Fprintf(w, `{"id":"%s","name":"Ada"}`, id)
	})
	empty := func(w http.ResponseWriter, req *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte("{}"))
	}
	for _, p := range []string{"/", "/health", "/version", "/items", "/items/{id}",
		"/orders", "/orders/{id}", "/posts", "/posts/{id}"} {
		r.Get(p, empty)
	}

	if err := http.ListenAndServe(addr, r); err != nil {
		fmt.Fprintln(os.Stderr, "listen:", err)
		os.Exit(1)
	}
}
