package router

// Route describes a single URL route in the service.
type Route struct {
	Path string
	Name string
}

// Routes is the canonical list of route strings. Note that "checkout"
// appears here only as a route name string, not as a handler definition.
var Routes = []Route{
	{Path: "/", Name: "home"},
	{Path: "/login", Name: "login"},
	{Path: "/checkout", Name: "checkout"},
}
