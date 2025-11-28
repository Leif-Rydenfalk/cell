package main

// This command runs 'cell-bind' to read the Rust schema for 'DadMsg'.
// It generates 'dad.go' with the DadMsg struct and strict serialization logic.
//
//go:generate cargo run -q -p cell-bind -- --lang go --cell DadMsg --out dad.go

import (
	"fmt"
	"log"
	"../../../../cell-go" // Local path for demo
)

func main() {
	fmt.Printf("Dad Schema Fingerprint: %x\n", DadMsg_Fingerprint)

	synapse, err := cell.Grow("dad")
	if err != nil {
		log.Fatalf("Failed to find dad: %v", err)
	}

	// 1. Create Message (Native Go Struct)
	msg := &DadMsg{
		A: 10,
		B: 32,
	}

	// 2. Serialize (Using generated packer)
	// This produces bytes compatible with Rust's rkyv layout for this struct.
	payload := msg.Serialize()

	// 3. Fire
	respBytes, err := synapse.Fire(payload)
	if err != nil {
		log.Fatal(err)
	}

	// 4. Deserialize Response
	// Dad replies with a DadMsg too
	reply := DeserializeDadMsg(respBytes)
	
	fmt.Printf("Dad says: %d + %d = %d\n", msg.A, msg.B, reply.A)
}