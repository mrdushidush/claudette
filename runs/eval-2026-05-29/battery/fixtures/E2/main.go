package main

import "fmt"

func main() {
	alice := User{First: "Alice", Last: "Adams"}
	bob := User{First: "Bob", Last: "Brown"}

	fmt.Println(GetUserName(alice))
	fmt.Println(GetUserName(bob))
}
