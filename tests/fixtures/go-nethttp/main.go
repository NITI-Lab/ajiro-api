package main

import (
	"log"
	"net/http"
)

const healthPath = "/healthz"

func main() {
	mux := http.NewServeMux()

	mux.HandleFunc("GET /users/{id}", getUser)
	mux.HandleFunc("POST /users", createUser)
	mux.HandleFunc("DELETE /users/{id}", deleteUser)
	mux.HandleFunc("GET /files/{path...}", serveFile)
	mux.HandleFunc("GET /{$}", index)
	mux.HandleFunc("POST api.example.com/webhooks", webhook)
	mux.HandleFunc("/legacy/", legacy)
	mux.HandleFunc(healthPath, health)
	mux.Handle("/static/", http.FileServer(http.Dir("static")))
	mux.Handle("GET /metrics", http.HandlerFunc(metrics))
	mux.HandleFunc("GET /closure", func(w http.ResponseWriter, r *http.Request) {})

	api := http.NewServeMux()
	api.HandleFunc("GET /items", listItems)
	api.HandleFunc("GET /items/{itemID}", getItem)
	mux.Handle("/api/", http.StripPrefix("/api", api))

	registerAdmin(mux)

	http.HandleFunc("/ping", ping)

	log.Fatal(http.ListenAndServe(":8080", mux))
}
