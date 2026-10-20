"""Exercise both real clients through lost and malformed isolated daemon IPC."""
import fcntl
import json
import os
import pty
import select
import socket
import struct
import subprocess
import sys
import termios
import threading
import time
from pathlib import Path

import pyte
from control_client import request
from inspection_client import Inspection


def read_frame(stream):
    def read(size):
        data = b""
        while len(data) < size:
            chunk = stream.recv(size - len(data))
            if not chunk:
                raise EOFError("IPC disconnected")
            data += chunk
        return data
    header = read(4)
    size = struct.unpack(">I", header)[0]
    if size > 32 * 1024 * 1024:
        raise ValueError("Unbounded fixture frame")
    return header + read(size)


class FaultProxy:
    """Only interpose on the private socket supplied by companion_fixture."""
    def __init__(self, path):
        self.path = Path(path)
        self.upstream = self.path.with_name("recovery.sock")
        if self.upstream.exists():
            raise RuntimeError("Refusing to replace an existing recovery socket")
        self.mode = "healthy"
        self.done = threading.Event()
        self.errors = []
        self.listener = socket.socket(socket.AF_UNIX)
        self.path.rename(self.upstream)
        try:
            self.listener.bind(str(self.path))
            self.path.chmod(0o600)
            self.listener.listen(16)
            self.listener.settimeout(.1)
        except BaseException:
            self.listener.close()
            self.path.unlink(missing_ok=True)
            self.upstream.rename(self.path)
            raise
        self.thread = threading.Thread(target=self.run)
        self.thread.start()

    def run(self):
        while not self.done.is_set():
            try:
                client, _ = self.listener.accept()
            except socket.timeout:
                continue
            with client:
                try:
                    client.settimeout(2)
                    message = read_frame(client)
                    if self.mode == "disconnected":
                        continue
                    if self.mode == "malformed":
                        client.sendall(struct.pack(">I", 8) + b"not-json")
                        continue
                    with socket.socket(socket.AF_UNIX) as backend:
                        backend.settimeout(3)
                        backend.connect(str(self.upstream))
                        backend.sendall(message)
                        client.sendall(read_frame(backend))
                except (BrokenPipeError, ConnectionResetError, EOFError):
                    pass
                except Exception as error:
                    self.errors.append(repr(error))

    def close(self):
        self.done.set()
        self.thread.join(timeout=5)
        self.listener.close()
        assert not self.thread.is_alive(), "Fixture proxy failed to stop"
        self.path.unlink(missing_ok=True)
        self.upstream.rename(self.path)


out = Path(sys.argv[2])
out.mkdir(parents=True, exist_ok=True)
session = json.loads(Path(os.environ["CLIPSYNC_UI_SESSION"]).read_text())
if sys.platform == "darwin":
    assert session["pasteboard"].startswith("org.clipsync.test.")
else:
    assert os.environ.get("CLIPSYNC_DISPOSABLE_DESKTOP") == "1"
ui = Inspection(int(os.environ.get("CLIPSYNC_UI_PORT", "18762")))
ui.click("Overview")
ui.wait(lambda: "Ready to sync" in ui.text(), "Initial GUI status")
master, slave = pty.openpty()
before = termios.tcgetattr(slave)
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
screen = pyte.Screen(100, 30)
stream = pyte.Stream(screen)
raw = bytearray()
process = None
proxy = None


def terminal_text():
    while select.select([master], [], [], .03)[0]:
        data = os.read(master, 65536)
        raw.extend(data)
        stream.feed(data.decode("utf-8", errors="replace"))
    return "\n".join(screen.display)


def wait_both(label):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        if label in terminal_text() and label in ui.text():
            return
        time.sleep(.1)
    raise AssertionError(f"Both clients must show {label}: {terminal_text()}\n{ui.text()}")


try:
    proxy = FaultProxy(session["socket"])
    process = subprocess.Popen(
        [sys.argv[1], "--config", session["config"], "tui"],
        stdin=slave, stdout=slave, stderr=slave, start_new_session=True,
        env=os.environ | {"TERM": "xterm-256color", "NO_COLOR": "1"},
    )
    wait_both("Ready to sync")
    for mode in ("disconnected", "malformed"):
        proxy.mode = mode
        wait_both("Service unavailable")
        ui.screenshot(out / f"{mode}.png")
        out.joinpath(f"{mode}.txt").write_text(terminal_text())
        # Actual keyboard/mouse actions while unavailable must not be deferred.
        os.write(master, b"p")
        ui.click("Pause syncing")
        terminal_text()
        proxy.mode = "healthy"
        wait_both("Ready to sync")
        status = request(session, "Status")
        assert status["status"]["paused"] is False, status
    # Both clients converge on one authoritative state after recovery.
    os.write(master, b"p")
    wait_both("Paused")
    ui.click("Resume syncing")
    wait_both("Ready to sync")
    assert not proxy.errors, proxy.errors
    os.write(master, b"q")
    process.wait(timeout=5)
    terminal_text()
    assert process.returncode == 0
    assert termios.tcgetattr(slave) == before, "termios not restored"
    assert b"\x1b[?1049l" in raw and b"\x1b[?25h" in raw
    result = {"passed": [
        "GUI and TUI reject disconnected and malformed IPC",
        "unavailable pause actions are not replayed after recovery",
        "both interfaces recover without relaunch",
        "TUI pause and GUI resume converge through the same daemon",
        "terminal state restored after error and recovery",
    ]}
    out.joinpath("results.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))
finally:
    if process and process.poll() is None:
        process.terminate()
        process.wait(timeout=5)
    if proxy:
        proxy.close()
    os.close(master)
    os.close(slave)
    ui.socket.close()
