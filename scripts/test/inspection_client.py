"""Local, test-feature-only eframe inspection client. No desktop clipboard access."""
import json
import socket
import struct
import time
import msgpack


class Inspection:
    def __init__(self, port):
        for _ in range(80):
            try:
                self.socket = socket.create_connection(('127.0.0.1', port), timeout=3)
                break
            except OSError:
                time.sleep(.15)
        else:
            raise RuntimeError(f'No inspection listener on {port}')
        self.socket.settimeout(10)
        assert self.read(8) == b'eins\0\0\0\1'
        self.modifiers = dict(alt=False, ctrl=False, shift=False, mac_cmd=False, command=False)

    def read(self, size):
        result = b''
        while len(result) < size:
            chunk = self.socket.recv(size - len(result))
            if not chunk:
                raise EOFError('Inspection disconnected')
            result += chunk
        return result

    def request(self, value):
        encoded = msgpack.packb(value, use_bin_type=True)
        self.socket.sendall(struct.pack('>I', len(encoded)) + encoded)
        size = struct.unpack('>I', self.read(4))[0]
        assert size < 64 * 1024 * 1024
        result = msgpack.unpackb(self.read(size), raw=False, strict_map_key=False)
        assert not isinstance(result, dict) or 'Error' not in result, result
        return result

    def tree(self):
        return self.request('GetTree')['Tree']

    def text(self):
        return json.dumps(self.tree(), default=lambda x: list(x))

    def wait(self, predicate, message, seconds=10):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if predicate():
                return
            time.sleep(.15)
        raise AssertionError(message + ': ' + self.text())

    def find(self, label):
        for _, node in self.tree()[2][0]:
            for value in node:
                if isinstance(value, dict) and str(value.get('label', '')).strip() == label.strip() and 'bounds' in value:
                    return value['bounds']
        raise AssertionError(f'No control {label}: {self.text()}')

    def enabled(self, label):
        for _, node in self.tree()[2][0]:
            if node[0] != 'button':
                continue
            for value in node:
                if isinstance(value, dict) and value.get('label') == label:
                    # The pinned AccessKit 0.25 Node encodes Disabled as flag 9.
                    return not node[3] & (1 << 9)
        return False

    def events(self, events):
        return self.request({'ApplyEvents': [events]})

    def click(self, label):
        for _ in range(14):
            x, y, right, bottom = self.find(label)
            if y >= 110 and bottom <= 790:
                break
            self.events([{'PointerMoved': [(x + right) / 2, 500]}, {'MouseWheel': dict(unit='Point', delta=[0, -260 if bottom > 790 else 260], phase='Move', source='Wheel', modifiers=self.modifiers)}])
            time.sleep(.15)
        x, y, right, bottom = self.find(label)
        position = [(x + right) / 2, (y + bottom) / 2]
        assert 0 <= position[1] < 800
        self.click_at(position)

    def click_at(self, position):
        # Keep move/press/release together across inspection frames.
        self.events([
            {'PointerMoved': position},
            *[
                {'PointerButton': dict(pos=position, button='Primary', pressed=pressed, modifiers=self.modifiers)}
                for pressed in (True, False)
            ],
        ])

    def screenshot(self, path):
        path.write_bytes(self.request({'GetScreenshot': [1.0]})['Screenshot'][1])
