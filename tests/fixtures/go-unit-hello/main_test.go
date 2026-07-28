package main

import "testing"

func TestMessage(t *testing.T) {
	if got := Message(); got != "hello from go-unit: Hello, world." {
		t.Fatalf("Message() = %q", got)
	}
}
