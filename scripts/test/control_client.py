"""Control protocol helper for isolated fixtures, with bounded frames/timeouts."""
import json
import socket
import struct


def request(session, command):
    with socket.socket(socket.AF_UNIX) as stream:
        stream.settimeout(8)
        stream.connect(session['socket'])
        payload = json.dumps({'version': 1, 'command': command}).encode()
        stream.sendall(struct.pack('>I', len(payload)) + payload)
        def read(size):
            data = b''
            while len(data) < size:
                chunk = stream.recv(size - len(data))
                if not chunk:
                    raise EOFError('Control closed')
                data += chunk
            return data
        size = struct.unpack('>I', read(4))[0]
        assert size <= 32 * 1024 * 1024
        response = json.loads(read(size))
        assert not response.get('error'), response
        return response
