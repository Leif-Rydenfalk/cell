import os
import socket
import struct
import json
import sys

class Membrane:
    def __init__(self):
        # 1. Socket Activation
        # In production (Daemon), we inherit the FD.
        fd_str = os.environ.get("CELL_SOCKET_FD")
        
        if fd_str:
            fd = int(fd_str)
            # socket.fromfd is crucial here. It wraps the raw file descriptor 
            # passed by the Rust parent process.
            self.listener = socket.fromfd(fd, socket.AF_UNIX, socket.SOCK_STREAM)
            # The socket is already bound by the Rust daemon, we just listen.
            self.listener.listen(1)
        else:
            # Fallback for manual testing (no daemon)
            sock_path = os.environ.get("CELL_SOCKET_PATH", "run/cell.sock")
            if os.path.exists(sock_path):
                os.remove(sock_path)
            self.listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            self.listener.bind(sock_path)
            self.listener.listen(1)
            print(f"[Python] Bound to {sock_path} (Dev Mode)")

    def bind(self, handler):
        """
        Blocking event loop.
        """
        sys.stderr.write("[Python] Membrane Active.\n")
        sys.stderr.flush()
        
        while True:
            try:
                conn, _ = self.listener.accept()
                self._handle_transport(conn, handler)
            except Exception as e:
                sys.stderr.write(f"[Python] Accept Error: {e}\n")

    def _handle_transport(self, conn, handler):
        try:
            while True:
                # 1. Read Length (4 bytes, Big Endian)
                header = self._recv_exact(conn, 4)
                if not header: break # EOF
                length = struct.unpack(">I", header)[0]

                # 2. Read Payload
                payload = self._recv_exact(conn, length)
                if payload is None: break

                # 3. Handle System Signals
                if payload == b"__GENOME__":
                    # Python doesn't have static schemas yet, return dynamic indicator
                    self._send_vesicle(conn, b'{"type":"dynamic_python"}')
                    continue

                # 4. Invoke User Handler
                try:
                    # Deserialize (Assume JSON for polyglot)
                    req_data = json.loads(payload.decode('utf-8'))
                    
                    # Call Logic
                    res_data = handler(req_data)
                    
                    # Serialize
                    res_bytes = json.dumps(res_data).encode('utf-8')
                    self._send_vesicle(conn, res_bytes)
                except json.JSONDecodeError:
                    sys.stderr.write("[Python] Error: Received non-JSON payload\n")
                    break
                except Exception as e:
                    sys.stderr.write(f"[Python] Handler Panic: {e}\n")
                    break
        finally:
            conn.close()

    def _send_vesicle(self, conn, data):
        length = len(data)
        # Pack length as Big Endian U32
        conn.sendall(struct.pack(">I", length))
        conn.sendall(data)

    def _recv_exact(self, conn, n):
        data = b''
        while len(data) < n:
            packet = conn.recv(n - len(data))
            if not packet: return None
            data += packet
        return data

class Synapse:
    def __init__(self, target_cell):
        golgi_path = os.environ.get("CELL_GOLGI_SOCK", "run/golgi.sock")
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(golgi_path)
        
        # Handshake: [0x01] [Len] [Name]
        self.sock.sendall(b'\x01')
        name_bytes = target_cell.encode('utf-8')
        self.sock.sendall(struct.pack(">I", len(name_bytes)))
        self.sock.sendall(name_bytes)
        
        ack = self.sock.recv(1)
        if ack != b'\x00':
            raise Exception(f"Golgi rejected connection to {target_cell}")

    def fire(self, data):
        # Fire JSON
        payload = json.dumps(data).encode('utf-8')
        length = len(payload)
        self.sock.sendall(struct.pack(">I", length))
        self.sock.sendall(payload)
        
        # Read Response
        header = self._recv_exact(4)
        resp_len = struct.unpack(">I", header)[0]
        resp_bytes = self._recv_exact(resp_len)
        
        return json.loads(resp_bytes.decode('utf-8'))

    def _recv_exact(self, n):
        data = b''
        while len(data) < n:
            packet = self.sock.recv(n - len(data))
            if not packet: raise Exception("Connection closed")
            data += packet
        return data