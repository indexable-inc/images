package main

import (
	"fmt"

	"rsc.io/quote/v4"
)

func Message() string {
	return "hello from go-unit: " + quote.Hello()
}

func main() {
	fmt.Println(Message())
}
