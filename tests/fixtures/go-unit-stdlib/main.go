package main

import (
	"fmt"
	"strings"
)

func Message() string {
	return strings.ToUpper("hello from go-unit stdlib")
}

func main() {
	fmt.Println(Message())
}
