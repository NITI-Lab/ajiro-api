package main

import "net/http"

func testSetup() {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /should-be-ignored", getUser)
}
