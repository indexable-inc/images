package main

import "testing"

func TestMessage(t *testing.T) {
	if got := Message(); got != "HELLO FROM GO-UNIT STDLIB" {
		t.Fatalf("Message() = %q", got)
	}
}
