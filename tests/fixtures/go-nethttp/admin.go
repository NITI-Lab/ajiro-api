package main

import "net/http"

func registerAdmin(mux *http.ServeMux) {
	mux.HandleFunc("GET /admin/stats", adminStats)
	mux.HandleFunc("PUT /admin/settings/{key}", adminSetting)
}
