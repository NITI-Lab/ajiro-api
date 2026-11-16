package main

import (
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/go-chi/chi/v5/middleware"
)

func main() {
	r := chi.NewRouter()
	r.Use(middleware.Logger)

	r.Get("/", home)
	r.Get("/health", health)
	r.Method("GET", "/version", http.HandlerFunc(version))
	r.MethodFunc("POST", "/echo", echo)

	r.Route("/users", func(r chi.Router) {
		r.Get("/", listUsers)
		r.Post("/", createUser)
		r.Route("/{id:[0-9]+}", func(r chi.Router) {
			r.Get("/", getUser)
			r.Put("/", updateUser)
			r.Delete("/", deleteUser)
			r.Get("/posts", listUserPosts)
		})
	})

	r.Group(func(r chi.Router) {
		r.Use(authMiddleware)
		r.Get("/me", me)
		r.With(rateLimit).Post("/me/avatar", uploadAvatar)
	})

	r.Mount("/admin", adminRouter())

	reports := chi.NewRouter()
	reports.Get("/daily", dailyReport)
	reports.Patch("/{id}", patchReport)
	r.Mount("/reports", reports)

	r.Handle("/assets/*", http.FileServer(http.Dir("assets")))

	http.ListenAndServe(":3000", r)
}
