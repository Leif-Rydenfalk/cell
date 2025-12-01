// SPDX-License-Identifier: MIT
// Copyright (c) 2025 Leif Rydenfalk – https://github.com/Leif-Rydenfalk/cell

package cell

import (
	"encoding/binary"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"time"
)

// --- Synapse (Client) ---

type Synapse struct {
	conn net.Conn
}

func Grow(targetCell string) (*Synapse, error) {
	socketPath := resolveSocketPath(targetCell)
	var conn net.Conn
	var err error

	// Germination wait loop
	for i := 0; i < 10; i++ {
		conn, err = net.Dial("unix", socketPath)
		if err == nil {
			break
		}
		time.Sleep(100 * time.Millisecond)
	}
	if err != nil {
		return nil, fmt.Errorf("failed to connect to %s: %v", targetCell, err)
	}

	return &Synapse{conn: conn}, nil
}

// Fire sends a raw byte slice (serialized vesicle) and returns the response.
func (s *Synapse) Fire(payload []byte) ([]byte, error) {
	// 1. Send Length
	lenBuf := make([]byte, 4)
	binary.LittleEndian.PutUint32(lenBuf, uint32(len(payload)))
	if _, err := s.conn.Write(lenBuf); err != nil {
		return nil, err
	}

	// 2. Send Payload
	if _, err := s.conn.Write(payload); err != nil {
		return nil, err
	}

	// 3. Read Response Length
	if _, err := io.ReadFull(s.conn, lenBuf); err != nil {
		return nil, err
	}
	respLen := binary.LittleEndian.Uint32(lenBuf)

	// 4. Read Response
	respBuf := make([]byte, respLen)
	if _, err := io.ReadFull(s.conn, respBuf); err != nil {
		return nil, err
	}

	return respBuf, nil
}

// --- Membrane (Server) ---

type Handler func([]byte) ([]byte, error)

type Membrane struct {
	name     string
	listener net.Listener
}

func Bind(name string, handler Handler) error {
	socketPath := resolveSocketPath(name)
	os.MkdirAll(filepath.Dir(socketPath), 0755)
	os.Remove(socketPath)

	l, err := net.Listen("unix", socketPath)
	if err != nil {
		return err
	}
	defer l.Close()

	fmt.Printf("[Go] Membrane '%s' Active at %s\n", name, socketPath)

	for {
		conn, err := l.Accept()
		if err != nil {
			continue
		}
		go handleConn(conn, handler)
	}
}

func handleConn(c net.Conn, h Handler) {
	defer c.Close()
	lenBuf := make([]byte, 4)

	for {
		// Read Length
		if _, err := io.ReadFull(c, lenBuf); err != nil {
			return
		}
		length := binary.LittleEndian.Uint32(lenBuf)

		// Read Payload
		buf := make([]byte, length)
		if _, err := io.ReadFull(c, buf); err != nil {
			return
		}

		// Handle
		resp, err := h(buf)
		if err != nil {
			return
		}

		// Write Response
		binary.LittleEndian.PutUint32(lenBuf, uint32(len(resp)))
		c.Write(lenBuf)
		c.Write(resp)
	}
}

func resolveSocketPath(name string) string {
	if dir := os.Getenv("CELL_SOCKET_DIR"); dir != "" {
		return filepath.Join(dir, name+".sock")
	}
	// Default matching Rust SDK
	return filepath.Join("/tmp/cell", name+".sock")
}