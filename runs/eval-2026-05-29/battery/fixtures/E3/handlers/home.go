package handlers

import (
	"fmt"
	"net/http"
)

// handleHome renders the landing page.
func handleHome(w http.ResponseWriter, r *http.Request) {
	fmt.Fprintln(w, "home")
}
