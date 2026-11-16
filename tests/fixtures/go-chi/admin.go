package main

import "github.com/go-chi/chi/v5"

func adminRouter() chi.Router {
	r := chi.NewRouter()
	r.Get("/stats", adminStats)
	r.Route("/users/{userID}", func(r chi.Router) {
		r.Delete("/", adminDeleteUser)
	})
	r.Mount("/audit", auditRouter())
	return r
}

func auditRouter() chi.Router {
	r := chi.NewRouter()
	r.Get("/", auditLog)
	return r
}
