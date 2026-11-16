package main

import (
	"net/http"

	"github.com/labstack/echo/v4"
	"github.com/labstack/echo/v4/middleware"
)

type Server struct {
	e *echo.Echo
}

func main() {
	e := echo.New()
	e.Use(middleware.Logger())

	e.GET("/", index)
	e.GET("/health", func(c echo.Context) error { return c.NoContent(http.StatusOK) })
	e.Any("/any", anyHandler)
	e.Match([]string{"GET", "POST"}, "/match", matchHandler)
	e.Add("PATCH", "/added", addedHandler)

	api := e.Group("/api", middleware.CORS())
	api.GET("/ping", ping)

	v1 := api.Group("/v1")
	v1.GET("/status", status)

	registerUsers(v1)

	h := &Handler{}
	h.Register(e.Group("/orders"))

	e.Logger.Fatal(e.Start(":8080"))
}
