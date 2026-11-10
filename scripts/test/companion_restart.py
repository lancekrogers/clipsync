"""Real daemon crash/restart and TUI recovery, using only disposable fixture state.

Usage: companion_restart.py FIXTURE_BINARY CLIPSYNC_BINARY EVIDENCE_DIRECTORY
On Linux run in a disposable X11/Wayland desktop, never the personal session.
"""
import fcntl
import hashlib
import json
import os
import pty
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
import uuid
from pathlib import Path

import pyte
from control_client import request


def stop(process):
    if process is not None and process.poll() is None:
        process.send_signal(signal.SIGINT)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def main():
    fixture_binary, cli, output = sys.argv[1:]
    out = Path(output)
    out.mkdir(parents=True, exist_ok=True)
    env = os.environ | {"TERM": "xterm-256color", "NO_COLOR": "1"}
    if sys.platform == "darwin":
        env["CLIPSYNC_TEST_PASTEBOARD"] = "org.clipsync.test." + str(uuid.uuid4())
    else:
        assert env.get("CLIPSYNC_DISPOSABLE_DESKTOP") == "1"
    fixture = terminal = None
    root = None
    master, slave = pty.openpty()
    before = termios.tcgetattr(slave)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
    screen = pyte.Screen(100, 30)
    stream = pyte.Stream(screen)
    raw = bytearray()

    def launch(*args):
        nonlocal fixture
        with (out / "fixture.log").open("a") as log:
            fixture = subprocess.Popen(
                [fixture_binary, *args], env=env, stdout=subprocess.PIPE,
                stderr=log, text=True,
            )
        assert select.select([fixture.stdout], [], [], 15)[0], "Fixture timeout"
        session = json.loads(fixture.stdout.readline())
        assert session["pid"] == fixture.pid
        return session

    def pump():
        if select.select([master], [], [], .05)[0]:
            data = os.read(master, 65536)
            raw.extend(data)
            stream.feed(data.decode("utf-8", errors="replace"))
        return "\n".join(screen.display)

    def wait_text(label):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            text = pump()
            if label in text:
                return text
            assert terminal.poll() is None, "TUI exited unexpectedly"
        raise AssertionError(f"Missing {label}: {text}")

    def snapshot():
        return {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                for p in root.iterdir() if p.is_file()}

    try:
        with tempfile.TemporaryDirectory() as unknown:
            rejected = subprocess.run([fixture_binary, "--resume", unknown],
                                      env=env, capture_output=True, timeout=10)
            assert rejected.returncode != 0
            assert not list(Path(unknown).iterdir()), "Unmarked directory modified"
        session = launch()
        root = Path(session["config"]).parent
        original = request(session, "Status")
        history = request(session, {"HistoryList": {"limit": 20}})["history"]
        files = snapshot()
        busy = subprocess.run([fixture_binary, "--resume", str(root)], env=env,
                              capture_output=True, timeout=10)
        assert busy.returncode != 0 and b"already owns" in busy.stderr
        assert snapshot() == files, "Live resume changed fixture state"
        terminal = subprocess.Popen([cli, "--config", session["config"], "tui"],
                                    stdin=slave, stdout=slave, stderr=slave,
                                    env=env, start_new_session=True)
        wait_text("Ready to sync")
        request(session, {"SetPause": {"paused": True}})
        wait_text("Paused")
        fixture.kill()
        fixture.wait(timeout=5)
        (out / "unavailable.txt").write_text(wait_text("Service unavailable"))

        # A symlink must not redirect retained history-key access elsewhere.
        key = root / "history.key"
        saved = root / "history.key.saved"
        key.rename(saved)
        try:
            key.symlink_to(saved)
            rejected = subprocess.run([fixture_binary, "--resume", str(root)],
                                      env=env, capture_output=True, timeout=10)
            assert rejected.returncode != 0 and b"Unsafe fixture file" in rejected.stderr
        finally:
            key.unlink()
            saved.rename(key)
        session_after = launch("--resume", str(root))
        assert session_after["config"] == session["config"]
        assert session_after["socket"] == session["socket"]
        status = request(session_after, "Status")
        assert status["pid"] != original["pid"]
        assert status["instance_id"] != original["instance_id"]
        assert not status["status"]["paused"], "Pause must reset on service restart"
        for name in ("identity", "history.key"):
            assert snapshot()[name] == files[name], f"{name} changed"
        assert request(session_after, {"HistoryList": {"limit": 20}})["history"] == history
        (out / "recovered.txt").write_text(wait_text("Ready to sync"))
        os.write(master, b"p")
        wait_text("Paused")
        assert request(session_after, "Status")["status"]["paused"]
        os.write(master, b"q")
        terminal.wait(timeout=5)
        pump()
        assert terminal.returncode == 0
        assert termios.tcgetattr(slave) == before
        assert b"\x1b[?1049l" in raw and b"\x1b[?25h" in raw
        assert request(session_after, "Status")["pid"] == session_after["pid"]
        result = {"passed": [
            "unmarked, live and symlinked fixture resumes rejected safely",
            "SIGKILL and new daemon PID/instance on the same config/socket",
            "identity and history key unchanged; encrypted history still readable",
            "live TUI detects unavailable service and recovers without relaunch",
            "pause resets on daemon restart; recovered TUI actions reach new daemon",
            "TUI exit restores terminal and leaves daemon running",
        ]}
        (out / "results.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result))
    finally:
        stop(terminal)
        stop(fixture)
        os.close(master)
        os.close(slave)
        if root and root.exists():
            # Only the directory just generated by this test's child fixture.
            shutil.rmtree(root)


if __name__ == "__main__":
    main()
