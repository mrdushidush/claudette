package handlers

import "net/http"

// Register wires every route to its handler. The "/checkout" route string
// is mentioned here, but the handler function itself is defined in another file.
func Register(mux *http.ServeMux) {
	mux.HandleFunc("/", handleHome)
	mux.HandleFunc("/login", handleLogin)
	mux.HandleFunc("/checkout", handleCheckout)
}
