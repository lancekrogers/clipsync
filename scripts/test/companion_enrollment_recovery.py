"""Pairing by code in the real TUI (and native GUI when present), with every
refusal checked before any trust write.

A is the joining computer; peer B is a second isolated fixture that allows a
device through its own control socket, as its companion would. With
CLIPSYNC_UI_SESSION set (fixture/app A started by companion-macos.py or
companion-linux.sh, inspection on 18762) the GUI half runs too. Without it this
harness starts fixture A itself and drives only the TUI, so it runs on a
developer Mac: private pasteboards, temporary stores, loopback, no GUI.

Settings is never opened. No discovery, clipboard or service action.
"""
import fcntl
import json
import os
import pty
import select
import signal
import socket
import struct
import subprocess
import sys
import termios
import time
import uuid
from pathlib import Path

import pyte
from control_client import request as control

root = Path(__file__).resolve().parents[2]
out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)
target = Path(os.environ.get("CARGO_TARGET_DIR", root / "target"))
binary = target / "debug" / "clipsync"
fixture_bin = target / "debug" / "examples" / "companion_fixture"
gui = bool(os.environ.get("CLIPSYNC_UI_SESSION"))

base_env = os.environ.copy()
for name in ("EGUI_INSPECTION", "CLIPSYNC_UI_SESSION", "CLIPSYNC_PEER_FIXTURE"):
    base_env.pop(name, None)
if sys.platform.startswith("linux"):
    assert os.environ.get("CLIPSYNC_DISPOSABLE_DESKTOP") == "1"
elif sys.platform != "darwin":
    raise SystemExit("Unsupported platform")

processes = []
logs = []


def start_fixture(name):
    env = base_env | {"CLIPSYNC_FIXTURE_NAME": name}
    if sys.platform == "darwin":
        env["CLIPSYNC_TEST_PASTEBOARD"] = "org.clipsync.test." + str(uuid.uuid4())
    log = out.joinpath(name.lower().replace(" ", "-") + ".log").open("w")
    logs.append(log)
    proc = subprocess.Popen([str(fixture_bin)], env=env, stdout=subprocess.PIPE, stderr=log, text=True)
    processes.append(proc)
    if not select.select([proc.stdout], [], [], 15)[0]:
        raise AssertionError(f"{name} fixture did not start; see {log.name}")
    return json.loads(proc.stdout.readline())


ui = tui = None
master = slave = None
raw = bytearray()
screen = pyte.Screen(110, 44)
stream = pyte.Stream(screen)
before = None
a_session = b_session = None
recorded = {}


def enroll(session):
    return control(session, {"EnrollmentInspect": {"session_id": None}})["enrollment"]


def pins(session):
    path = Path(session["config"]).parent / "authorized_devices"
    if not path.exists():
        return None
    return path.read_bytes(), path.stat().st_mode & 0o777


def trusted(session):
    return sorted(row["fingerprint"] for row in control(session, "TrustedDevices")["trusted_devices"])


def until(predicate, message, seconds=15):
    deadline = time.monotonic() + seconds
    last = None
    while time.monotonic() < deadline:
        last = predicate()
        if last:
            return last
        time.sleep(0.15)
    raise AssertionError(message + ": " + repr(last))


def released_loopback():
    bound = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    bound.bind(("127.0.0.1", 0))
    port = bound.getsockname()[1]
    bound.close()
    return f"127.0.0.1:{port}"


def allow(session):
    snap = control(session, "EnrollmentOpen")["enrollment"]
    return snap["listen_addr"], snap["code"]["code"]


def live_code(session):
    snap = enroll(session)
    return snap["code"]["code"] if snap.get("code") else None


def wrong(code):
    return f"{(int(code) + 1) % 1_000_000:06d}"


def pump(duration=0.3):
    end = time.monotonic() + duration
    while time.monotonic() < end:
        if not select.select([master], [], [], 0.03)[0]:
            continue
        try:
            data = os.read(master, 65536)
        except OSError:
            return
        if not data:
            return
        raw.extend(data)
        stream.feed(data.decode("utf-8", errors="replace"))


def terminal():
    pump(0.05)
    return "\n".join(line.rstrip() for line in screen.display).rstrip() + "\n"


def see(marker, seconds=15, absent=()):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        pump(0.2)
        current = terminal()
        if marker in current and all(item not in current for item in absent):
            return current
    raise AssertionError("TUI missing %r\n%s" % (marker, terminal()))


def keys(data, settle=0.25):
    os.write(master, data)
    pump(settle)


def tui_pair(address, code):
    erase = termios.tcgetattr(slave)[6][termios.VERASE]
    if isinstance(erase, int):
        erase = bytes([erase])
    keys(b"m")
    see("Address of the other computer")
    keys(erase * 48 + address.encode() + b"\r")
    see("Code shown on")
    keys(code.encode())
    keys(b"\r", settle=0.5)


def unchanged(a_base, b_base):
    assert pins(a_session) == a_base, "A authorized_devices changed"
    assert pins(b_session) == b_base, "B authorized_devices changed"


def save(name):
    out.joinpath(f"{name}.txt").write_text(terminal())
    if ui:
        ui.screenshot(out / f"{name}.png")


def set_field(view, index, text):
    """Type into the index-th text field (0 = code, 1 = address)."""
    view.type_into(index, text)


def gui_pair(address, code):
    set_field(ui, 0, code)
    set_field(ui, 1, address)
    ui.wait(lambda: ui.enabled("Pair with address"), "GUI pair ready", seconds=12)
    ui.click("Pair with address")


try:
    if gui:
        a_session = json.loads(Path(os.environ["CLIPSYNC_UI_SESSION"]).read_text())
    else:
        a_session = start_fixture("Isolated test Mac")
    b_session = start_fixture("Peer test Mac")
    for session in (a_session, b_session):
        features = control(session, "Status")["features"]
        if "enrollment_code" not in features:
            raise SystemExit("These binaries predate code pairing; rebuild before running.")
    a_base, b_base = pins(a_session), pins(b_session)
    if gui:
        from inspection_client import Inspection
        ui = Inspection(int(os.environ.get("CLIPSYNC_UI_PORT", "18762")))
        ui.click("Devices")
        ui.wait(lambda: ui.enabled("Allow a new device"), "pairing actions ready", seconds=12)

    master, slave = pty.openpty()
    before = termios.tcgetattr(slave)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 44, 110, 0, 0))
    tui = subprocess.Popen(
        [str(binary), "--config", a_session["config"], "tui"],
        stdin=slave, stdout=slave, stderr=slave, start_new_session=True,
        env=base_env | {"TERM": "xterm-256color", "NO_COLOR": "1", "CLIPSYNC_TUI_GRAPHICS": "off"},
    )
    see("Your clipboard, everywhere")
    keys(b"2")
    see("Authorized devices")
    see("To pair, press e on one computer")

    # Nothing is listening at a released port.
    closed = released_loopback()
    tui_pair(closed, "123456")
    see("Nothing is accepting pairing at that address")
    unchanged(a_base, b_base)
    save("not-listening-tui")
    recorded["tui_not_listening"] = closed

    # A wrong code is refused on both computers and the allowing side moves on.
    listen, code = allow(b_session)
    assert listen.startswith("127.0.0.1:"), listen
    tui_pair(listen, wrong(code))
    see("That code did not match")
    until(lambda: enroll(b_session)["outcome"] and enroll(b_session)["outcome"]["status"] == "wrong_code", "B saw the wrong code")
    fresh = until(lambda: live_code(b_session) not in (None, code) and live_code(b_session), "B shows a new code")
    unchanged(a_base, b_base)
    save("wrong-code-tui")
    recorded["tui_wrong_code"] = {"attempt": "refused", "new_code_shown": True}

    if ui:
        closed_gui = released_loopback()
        gui_pair(closed_gui, "123456")
        ui.wait(lambda: "Nothing is accepting pairing at that address" in ui.text(), "GUI not listening", seconds=15)
        unchanged(a_base, b_base)
        save("not-listening-gui")
        gui_pair(listen, wrong(fresh))
        ui.wait(lambda: "That code did not match" in ui.text(), "GUI wrong code", seconds=15)
        fresh = until(lambda: live_code(b_session) not in (None, fresh) and live_code(b_session), "B renews after the GUI attempt")
        unchanged(a_base, b_base)
        save("wrong-code-gui")
        recorded["gui_rejections"] = ["not_listening", "wrong_code"]

    # While typing a code, digits stay in the field and q does not quit.
    keys(b"m")
    see("Address of the other computer")
    erase = termios.tcgetattr(slave)[6][termios.VERASE]
    erase = bytes([erase]) if isinstance(erase, int) else erase
    keys(erase * 48 + listen.encode() + b"\r")
    see("Code shown on")
    keys(b"1q34")
    screen_now = see("1 3 4", absent=("Browse clipboard history",))
    assert "Code shown on" in screen_now and "type the code" in screen_now, screen_now
    assert tui.poll() is None, "q quit while typing a code"
    keys(b"\x1b")
    see("To pair, press e on one computer", absent=("Code shown on",))
    unchanged(a_base, b_base)

    # The right code pairs, and both files gain exactly the other's key.
    tui_pair(listen, live_code(b_session))
    see("Paired. Both computers saved the pairing.")
    see("Peer test Mac")
    until(lambda: len(trusted(a_session)) == 1 and len(trusted(b_session)) == 1, "both saved")
    a_key = control(a_session, "TrustedDevices")["trusted_devices"]
    b_key = control(b_session, "TrustedDevices")["trusted_devices"]
    assert a_key[0]["comment"].startswith("Peer test Mac SHA256:"), a_key
    assert b_key[0]["comment"].startswith("Isolated test Mac SHA256:"), b_key
    assert a_key[0]["fingerprint"] in terminal()
    for session in (a_session, b_session):
        path = Path(session["config"]).parent / "authorized_devices"
        assert path.read_text().count("ssh-ed25519 ") == 1
        assert path.stat().st_mode & 0o077 == 0
    assert not enroll(b_session)["listening"], "B keeps allowing after pairing"
    save("paired-tui")
    recorded["paired"] = {"a_trusts": a_key[0]["comment"].split()[0:3], "b_trusts": b_key[0]["comment"].split()[0:3]}

    keys(b"q", settle=0.6)
    assert tui.wait(timeout=5) == 0
    pump(0.6)
    assert termios.tcgetattr(slave) == before, "termios not restored"
    assert b"\x1b[?1049l" in raw, "alternate screen not left"
    assert b"\x1b[?25h" in raw, "cursor not restored"

    for session in (a_session, b_session):
        for row in control(session, "TrustedDevices")["trusted_devices"]:
            control(session, {"EnrollmentRevoke": {"fingerprint": row["fingerprint"]}})
    result = {
        "passed": [
            "TUI pairing to a released loopback port says nothing is accepting pairing and writes no trust",
            "TUI wrong code is refused on both computers, the allowing side shows a new code, no trust written",
            *(["GUI not-listening and wrong-code refusals write no trust"] if ui else []),
            "digits and q stay in the code field while typing; Esc returns to Devices",
            "TUI right code pairs: each authorized_devices gains exactly the other key, mode 0600",
            "allowing window closes after pairing",
            "terminal modes, cursor, and alternate screen restored",
        ],
        "gui": bool(ui),
        **recorded,
    }
    out.joinpath("results.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))
finally:
    if tui and tui.poll() is None:
        tui.terminate()
        try:
            tui.wait(timeout=5)
        except subprocess.TimeoutExpired:
            tui.kill()
            tui.wait()
    for session in (a_session, b_session):
        if session:
            try:
                control(session, {"EnrollmentCancel": {"session_id": None}})
            except Exception:
                pass
    for proc in processes:
        if proc.poll() is None:
            proc.send_signal(signal.SIGINT)
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
    if ui:
        ui.socket.close()
    if master is not None:
        os.close(master)
    if slave is not None:
        os.close(slave)
    for log in logs:
        log.close()
