"""Delete and clear History in the real TUI against an isolated macOS fixture.

Usage: companion_history_removal_tui.py FIXTURE_BINARY CLIPSYNC_BINARY EVIDENCE_DIRECTORY

Checks both the screen and the fixture's own history over its control socket:
answering anything but y changes nothing, d then y removes one saved copy, and
D then y removes them all. Settings is never opened.
"""
import codecs
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
from control_client import request

if sys.platform != "darwin":
    raise SystemExit("Requires macOS; the fixture uses a private named pasteboard.")
fixture_binary, cli, output = sys.argv[1:]
out = Path(output)
out.mkdir(parents=True, exist_ok=True)
env = os.environ | {
    "CLIPSYNC_TEST_PASTEBOARD": "org.clipsync.test." + str(uuid.uuid4()),
    "CLIPSYNC_PEER_FIXTURE": "1",
    "CLIPSYNC_TUI_GRAPHICS": "off",
    "TERM": "xterm-256color",
}
env.pop("NO_COLOR", None)
env.pop("TERM_PROGRAM", None)
WIDTH, HEIGHT = 100, 30

fixture = subprocess.Popen(
    [fixture_binary], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
)
master = slave = tui = None
screen = pyte.Screen(WIDTH, HEIGHT)
stream = pyte.Stream(screen)
decoder = codecs.getincrementaldecoder("utf-8")("replace")


def pump(seconds=0.1):
    end = time.monotonic() + seconds
    while True:
        if select.select([master], [], [], 0.05)[0]:
            data = os.read(master, 65536)
            if b"\x1b[6n" in data:
                os.write(master, b"\x1b[1;1R" * data.count(b"\x1b[6n"))
            stream.feed(decoder.decode(data))
        if time.monotonic() >= end:
            return


def snapshot():
    return "\n".join(line.rstrip() for line in screen.display)


def wait_for(predicate, label, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        pump()
        if predicate(snapshot()):
            return
    raise AssertionError(f"{label}:\n{snapshot()}")


def save(name):
    out.joinpath(f"{name}.txt").write_text(snapshot() + "\n")


def send(keys):
    os.write(master, keys)
    pump(0.3)


def saved(session):
    entries = request(session, {"HistoryList": {"limit": 20}})["history"]["entries"]
    return [entry["preview"] for entry in entries]


def footer():
    return [line for line in snapshot().splitlines() if line.strip()][-2]


try:
    if not select.select([fixture.stdout], [], [], 15)[0]:
        raise RuntimeError("Fixture did not start")
    session = json.loads(fixture.stdout.readline())
    features = request(session, "Status")["features"]
    assert {"history_delete", "history_clear"} <= set(features), features
    before = saved(session)
    assert "teammate@example.com" in before and len(before) >= 3, before

    master, slave = pty.openpty()
    settings = termios.tcgetattr(slave)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", HEIGHT, WIDTH, 0, 0))
    tui = subprocess.Popen(
        [cli, "--config", session["config"], "tui"],
        stdin=slave, stdout=slave, stderr=slave, env=env, start_new_session=True,
    )
    wait_for(lambda text: "Connected · workbench" in text, "peer connected")
    send(b"3")
    wait_for(lambda text: "Lunch at 12:30" in text, "history rows")
    wait_for(lambda text: "d delete" in text and "D clear all" in text, "removal hints")
    # Enter selects the first row; the fixture's list order is the screen order.
    send(b"\r")
    for _ in range(before.index("teammate@example.com")):
        send(b"\x1b[B")
    wait_for(lambda text: text.count("teammate@example.com") >= 2, "selected row in the detail pane")
    save("01-selected")

    send(b"d")
    wait_for(lambda text: "Delete this saved copy" in text and "other keys cancel" in text, "delete question")
    save("02-delete-question")
    send(b"n")
    wait_for(lambda text: "Delete this saved copy" not in text, "delete question closed")
    pump(1)
    assert "teammate@example.com" in snapshot() and "Deleted" not in snapshot(), snapshot()
    assert saved(session) == before, "answering n removed history"

    send(b"d")
    wait_for(lambda text: "Delete this saved copy" in text, "delete question again")
    send(b"y")
    wait_for(lambda text: "Deleted" in text and "teammate@example.com" not in text, "row deleted")
    after_delete = saved(session)
    assert after_delete == [p for p in before if p != "teammate@example.com"], after_delete
    # The row that moved up into its place is selected and previewed.
    nxt = before[before.index("teammate@example.com") + 1][:24]
    wait_for(lambda text: text.count(nxt) >= 2, "neighbor selected after delete")
    pump(2)
    assert "Deleted" in snapshot(), "delete result vanished early:\n" + snapshot()
    delete_message = footer().strip()
    save("03-deleted")

    send(b"D")
    wait_for(lambda text: "Clear all history" in text, "clear question")
    save("04-clear-question")
    send(b"\x1b")
    wait_for(lambda text: "Clear all history" not in text, "clear question closed")
    pump(1)
    assert tui.poll() is None, "Esc during a question quit the app"
    assert "Lunch at 12:30" in snapshot(), snapshot()
    assert saved(session) == after_delete, "answering Esc removed history"

    send(b"D")
    wait_for(lambda text: "Clear all history" in text, "clear question again")
    send(b"Y")
    wait_for(
        lambda text: f"Cleared {len(after_delete)} saved copies" in text and "Nothing copied yet." in text,
        "history cleared",
    )
    assert saved(session) == [], "history remains after clear"
    pump(2)
    assert "Cleared" in snapshot(), "clear result vanished early:\n" + snapshot()
    clear_message = footer().strip()
    save("05-cleared")

    send(b"q")
    assert tui.wait(timeout=5) == 0
    assert termios.tcgetattr(slave) == settings, "terminal settings were not restored"
    results = {
        "passed": [
            "n and Esc cancel without removing anything",
            "d then y deletes the selected copy here and selects its neighbor",
            "D then Y clears all history",
            "results stay on screen",
            "terminal restored",
        ],
        "delete_message": delete_message,
        "clear_message": clear_message,
        "saved_before": len(before),
        "saved_after_delete": len(after_delete),
    }
    out.joinpath("results.json").write_text(json.dumps(results, indent=2) + "\n")
    print(json.dumps(results, indent=2))
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
