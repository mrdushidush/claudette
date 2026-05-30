package main

import "fmt"

// User holds a person's identity fields.
type User struct {
	First string
	Last  string
}

// GetUserName returns the user's full display name.
func GetUserName(u User) string {
	return fmt.Sprintf("%s %s", u.First, u.Last)
}
