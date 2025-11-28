import os
import struct
import socket
import json
import threading

class Membrane:
    def __init__(self, name):
        self.name = name
        self.socket_dir = os.environ.get("CELL_SOCKET_DIR", "/tmp/cell")
        self.socket_path = os.path.join(self.socket_dir, f"{name}.sock")
        self.running = True

    def bind(self, handler):
        if not os.path.exists(self.socket_dir):
            os.makedirs(self.socket_dir)
        
        if os.path.exists(self.socket_path):
            os.remove(self.socket_path)
            
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(self.socket_path)
        server.listen(10)
        
        print(f"[Python] Cell '{self.name}' Active.")
        
        try:
            while self.running:
                conn, _ = server.accept()
                threading.Thread(target=self._handle_client, args=(conn, handler)).start()
        except KeyboardInterrupt:
            pass
        finally:
            server.close()
            if os.path.exists(self.socket_path):
                os.remove(self.socket_path)

    def _handle_client(self, conn, handler):
        try:
            while True:
                # 1. Read Length (4 bytes LE)
                header = conn.recv(4)
                if not header or len(header) < 4:
                    break
                length = struct.unpack('<I', header)[0]
                
                # 2. Read Payload
                data = b''
                while len(data) < length:
                    chunk = conn.recv(length - len(data))
                    if not chunk: break
                    data += chunk
                
                # 3. Handle (Assume JSON for Python compatibility)
                req_obj = json.loads(data.decode('utf-8'))
                
                # 4. Invoke User Handler
                resp_obj = handler(req_obj)
                
                # 5. Send Response
                resp_bytes = json.dumps(resp_obj).encode('utf-8')
                conn.sendall(struct.pack('<I', len(resp_bytes)))
                conn.sendall(resp_bytes)
        except Exception as e:
            print(f"Error: {e}")
        finally:
            conn.close()

class Synapse:
    @staticmethod
    def call(target_name, payload):
        socket_dir = os.environ.get("CELL_SOCKET_DIR", "/tmp/cell")
        socket_path = os.path.join(socket_dir, f"{target_name}.sock")
        
        if not os.path.exists(socket_path):
            # In a real impl, we would tug the Umbilical Cord here.
            # For this MVP python wrapper, we assume the cell exists.
            raise ConnectionError(f"Cell {target_name} not found")

        client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        client.connect(socket_path)
        
        # Send
        payload_bytes = json.dumps(payload).encode('utf-8')
        client.sendall(struct.pack('<I', len(payload_bytes)))
        client.sendall(payload_bytes)
        
        # Receive
        header = client.recv(4)
        length = struct.unpack('<I', header)[0]
        data = b''
        while len(data) < length:
            data += client.recv(length - len(data))
            
        client.close()
        return json.loads(data.decode('utf-8'))

# Example Usage:
# def handler(req):
#     return {"status": "ok", "echo": req}
# Membrane("python-worker").bind(handler)