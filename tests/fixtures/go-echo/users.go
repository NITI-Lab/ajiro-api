package main

import "github.com/labstack/echo/v4"

func registerUsers(g *echo.Group) {
	users := g.Group("/users")
	users.GET("", listUsers)
	users.POST("", createUser)
	users.GET("/:id", getUser)
	users.DELETE("/:id", deleteUser)
}

type Handler struct{}

func (h *Handler) Register(g *echo.Group) {
	g.GET("", h.List)
	g.GET("/:orderID", h.Get)
	g.POST("/:orderID/cancel", h.Cancel)
}

func (h *Handler) List(c echo.Context) error   { return nil }
func (h *Handler) Get(c echo.Context) error    { return nil }
func (h *Handler) Cancel(c echo.Context) error { return nil }

func index(c echo.Context) error        { return nil }
func anyHandler(c echo.Context) error   { return nil }
func matchHandler(c echo.Context) error { return nil }
func addedHandler(c echo.Context) error { return nil }
func ping(c echo.Context) error         { return nil }
func status(c echo.Context) error       { return nil }
func listUsers(c echo.Context) error    { return nil }
func createUser(c echo.Context) error   { return nil }
func getUser(c echo.Context) error      { return nil }
func deleteUser(c echo.Context) error   { return nil }
