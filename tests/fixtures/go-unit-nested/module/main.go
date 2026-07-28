package main

import (
	"fmt"

	"rsc.io/quote/v4"
)

func Message() string {
	return "hello from nested go-unit: " + quote.Hello()
}

func main() {
	fmt.Println(Message())
}
