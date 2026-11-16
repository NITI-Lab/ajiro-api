package main

import "net/http"

func getUser(w http.ResponseWriter, r *http.Request)     {}
func createUser(w http.ResponseWriter, r *http.Request)  {}
func deleteUser(w http.ResponseWriter, r *http.Request)  {}
func serveFile(w http.ResponseWriter, r *http.Request)   {}
func index(w http.ResponseWriter, r *http.Request)       {}
func webhook(w http.ResponseWriter, r *http.Request)     {}
func legacy(w http.ResponseWriter, r *http.Request)      {}
func health(w http.ResponseWriter, r *http.Request)      {}
func metrics(w http.ResponseWriter, r *http.Request)     {}
func listItems(w http.ResponseWriter, r *http.Request)   {}
func getItem(w http.ResponseWriter, r *http.Request)     {}
func adminStats(w http.ResponseWriter, r *http.Request)  {}
func adminSetting(w http.ResponseWriter, r *http.Request) {}
func ping(w http.ResponseWriter, r *http.Request)        {}
