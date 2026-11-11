"""Exercise image history in the real TUI using an isolated macOS pasteboard."""
import codecs
import fcntl
import json
import os
import pty
import re
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
env.pop("TERM_PROGRAM", None)
kitty = os.environ.get("CLIPSYNC_TEST_KITTY") == "1"
if kitty:
    env["CLIPSYNC_IMAGE_DETAIL_FIXTURE"] = "1"
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
    answered = False
    cursor_queries = 0
    pending = bytearray()
    # Reads can split a multi-byte character; decode across them.
    decoder = codecs.getincrementaldecoder("utf-8")("replace")

    def pump():
        global answered, cursor_queries
        if select.select([master], [], [], 0.1)[0]:
            data = os.read(master, 65536)
            raw.extend(data)
            if kitty and not answered and b"\x1b[5n" in raw:
                os.write(master, b"\x1b_Gi=31;OK\x1b\\\x1b[6;20;10t\x1b[0n")
                answered = True
            count = raw.count(b"\x1b[6n")
            if count > cursor_queries:
                os.write(master, b"\x1b[1;1R" * (count - cursor_queries))
                cursor_queries = count
            # pyte does not implement Kitty's APC image transmission. Remove
            # complete graphics packets before feeding its text emulator.
            pending.extend(data)
            while pending:
                start = pending.find(b"\x1b_G")
                if start < 0:
                    keep = 2 if pending.endswith(b"\x1b_") else 1 if pending.endswith(b"\x1b") else 0
                    if len(pending) > keep:
                        stream.feed(decoder.decode(bytes(pending[:len(pending)-keep])))
                        del pending[:len(pending)-keep]
                    break
                if start:
                    stream.feed(decoder.decode(bytes(pending[:start])))
                    del pending[:start]
                end = pending.find(b"\x1b\\", 3)
                if end < 0:
                    break
                del pending[:end+2]

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
    wait_for(lambda: "▣ Image" in snapshot(), "image history row")
    os.write(master, b"\r")
    if kitty:
        wait_for(lambda: b",a=T,U=1,f=32" in raw, "Kitty graphics image")
        dimensions = re.search(rb"a=T,U=1,f=32,t=d,s=(\d+),v=(\d+)", raw)
        assert dimensions and int(dimensions[1]) > 64 and int(dimensions[2]) > 64, "graphics preview lost detail"
    else:
        wait_for(lambda: "▀" in snapshot(), "colored image preview")
        assert b"\x1b[38;2;" in raw or b"\x1b[48;2;" in raw, "missing RGB styling"
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 18, 60, 0, 0))
    screen.resize(18, 60)
    os.killpg(tui.pid, signal.SIGWINCH)
    if kitty:
        wait_for(lambda: raw.count(b",a=T,U=1,f=32") >= 2, "graphics image after resize")
    else:
        wait_for(lambda: "▀" in snapshot(), "image preview after resize")
    os.write(master, b"\x1b[B")
    # Text details title their character count; image details do not.
    wait_for(lambda: "▀" not in snapshot() and " characters" in snapshot(), "text replaces preview")
    assert "\U0010eeee" not in snapshot(), "stale graphics placeholder remains"
    os.write(master, b"q")
    assert tui.wait(timeout=5) == 0
    assert termios.tcgetattr(slave) == before, "terminal settings were not restored"
    print("Real TUI rendered " + ("Kitty graphics" if kitty else "RGB fallback") + " and cleared it after selection changed.")
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
