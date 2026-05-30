package main

import (
	"net/http"

	"shopsvc/handlers"
)

func main() {
	mux := http.NewServeMux()
	handlers.Register(mux)
	http.ListenAndServe(":8080", mux)
}
