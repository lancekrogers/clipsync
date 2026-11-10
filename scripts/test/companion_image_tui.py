"""Exercise image history in the real TUI using an isolated macOS pasteboard."""
import fcntl
import json
import os
import pty
import select
import signal
import struct
import subprocess
import sys
import termios
import time
import uuid
from pathlib import Path

import pyte

if sys.platform != "darwin":
    raise SystemExit("Requires macOS; the fixture uses a private named pasteboard.")
root = Path(__file__).resolve().parents[2]
env = os.environ | {
    "CLIPSYNC_TEST_PASTEBOARD": "org.clipsync.test." + str(uuid.uuid4()),
    "CLIPSYNC_IMAGE_FIXTURE": "1",
    "TERM": "xterm-256color",
}
env.pop("NO_COLOR", None)
fixture = subprocess.Popen(
    [str(root / "target/debug/examples/companion_fixture")], env=env,
    stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
)
master = slave = None
tui = None
try:
    if not select.select([fixture.stdout], [], [], 15)[0]:
        raise RuntimeError("Fixture did not start")
    session = json.loads(fixture.stdout.readline())
    master, slave = pty.openpty()
    before = termios.tcgetattr(slave)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
    tui = subprocess.Popen(
        [str(root / "target/debug/clipsync"), "--config", session["config"], "tui"],
        stdin=slave, stdout=slave, stderr=slave, env=env, start_new_session=True,
    )
    screen = pyte.Screen(100, 30)
    stream = pyte.Stream(screen)
    raw = bytearray()

    def pump():
        if select.select([master], [], [], 0.1)[0]:
            data = os.read(master, 65536)
            raw.extend(data)
            stream.feed(data.decode("utf-8", errors="replace"))

    def snapshot():
        return "\n".join(screen.display)

    def wait_for(predicate, label):
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline:
            pump()
            if predicate():
                return
        raise AssertionError(f"{label}:\n{snapshot()}")

    wait_for(lambda: "Ready to sync" in snapshot(), "service ready")
    os.write(master, b"3")
    wait_for(lambda: "[Image]" in snapshot(), "image history row")
    os.write(master, b"\r")
    wait_for(lambda: "▀" in snapshot(), "colored image preview")
    assert b"\x1b[38;2;" in raw or b"\x1b[48;2;" in raw, "missing RGB styling"
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 18, 60, 0, 0))
    screen.resize(18, 60)
    os.killpg(tui.pid, signal.SIGWINCH)
    wait_for(lambda: "▀" in snapshot(), "image preview after resize")
    os.write(master, b"\x1b[B")
    wait_for(lambda: "▀" not in snapshot() and "Selected copy" in snapshot(), "text replaces preview")
    os.write(master, b"q")
    assert tui.wait(timeout=5) == 0
    assert termios.tcgetattr(slave) == before, "terminal settings were not restored"
    print("Real TUI rendered an RGB image preview and cleared it after selection changed.")
finally:
    if tui is not None and tui.poll() is None:
        tui.terminate()
        tui.wait(timeout=5)
    for fd in (master, slave):
        if fd is not None:
            os.close(fd)
    if fixture.poll() is None:
        fixture.send_signal(signal.SIGINT)
        try:
            fixture.wait(timeout=5)
        except subprocess.TimeoutExpired:
            fixture.kill()
            fixture.wait()
