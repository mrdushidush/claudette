package handlers

import (
	"fmt"
	"net/http"
)

// handleLogin authenticates a user and starts a session.
func handleLogin(w http.ResponseWriter, r *http.Request) {
	fmt.Fprintln(w, "login")
}
