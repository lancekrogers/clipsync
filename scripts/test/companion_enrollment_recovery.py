"""GUI and TUI rejection of enrollment before any trust write.

Root already started fixture/app A (CLIPSYNC_UI_SESSION, inspection on 18762).
This process starts peer B only: a private macOS pasteboard, or the disposable
Linux guard. B's listener is opened through control. Every dial, cancel, and
address entry on A is a real GUI event or TUI keypress. No confirm, discovery,
clipboard, or service action.
"""
import fcntl
import json
import os
import pty
import re
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
from inspection_client import Inspection

FULL = re.compile(r"^SHA256:[A-Za-z0-9+/]{43}$")
root = Path(__file__).resolve().parents[2]
out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)
a_session = json.loads(Path(os.environ["CLIPSYNC_UI_SESSION"]).read_text())
if sys.platform == "darwin":
    assert str(a_session.get("pasteboard", "")).startswith("org.clipsync.test.")
    b_env = os.environ | {"CLIPSYNC_TEST_PASTEBOARD": "org.clipsync.test." + str(uuid.uuid4())}
elif sys.platform.startswith("linux"):
    assert os.environ.get("CLIPSYNC_DISPOSABLE_DESKTOP") == "1"
    b_env = os.environ.copy()
else:
    raise SystemExit("Unsupported platform")
b_env.pop("EGUI_INSPECTION", None)
b_env.pop("CLIPSYNC_UI_SESSION", None)

binary = root / "target" / "debug" / "clipsync"
fixture_bin = root / "target" / "debug" / "examples" / "companion_fixture"
peer_log = out.joinpath("peer-fixture.log").open("w")
b_proc = subprocess.Popen(
    [str(fixture_bin)], env=b_env, stdout=subprocess.PIPE, stderr=peer_log, text=True,
)
ui = tui = None
master = slave = None
raw = bytearray()
screen = pyte.Screen(100, 30)
stream = pyte.Stream(screen)
before = None
b_session = None
recorded = {}


def enroll(session):
    return control(session, {"EnrollmentInspect": {"session_id": None}})["enrollment"]


def auth_state(session):
    path = Path(session["config"]).parent / "authorized_devices"
    if not path.exists():
        return None
    return path.read_bytes(), path.stat().st_mode & 0o777


def trusted(session):
    rows = control(session, "TrustedDevices")["trusted_devices"]
    return sorted((row.get("fingerprint"), row.get("comment")) for row in rows)


def ids(session):
    return {item["session_id"] for item in enroll(session)["sessions"]}


def by_id(session, session_id):
    for item in enroll(session)["sessions"]:
        if item["session_id"] == session_id:
            return item
    raise AssertionError("missing session " + str(session_id))


def full(value, label):
    assert isinstance(value, str) and FULL.fullmatch(value), label + " " + repr(value)
    return value


def released_loopback():
    bound = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    bound.bind(("127.0.0.1", 0))
    port = bound.getsockname()[1]
    bound.close()
    probe = socket.socket()
    probe.settimeout(0.2)
    try:
        refused = probe.connect_ex(("127.0.0.1", port))
    finally:
        probe.close()
    if refused == 0:
        raise AssertionError(f"127.0.0.1:{port} accepted a connection")
    return f"127.0.0.1:{port}"


def until(predicate, message, seconds=12):
    deadline = time.monotonic() + seconds
    last = None
    while time.monotonic() < deadline:
        last = predicate()
        if last:
            return last
        time.sleep(0.15)
    raise AssertionError(message + ": " + repr(last))


def await_dial(addr, prior, ready):
    def match():
        found = [
            item for item in enroll(a_session)["sessions"]
            if item["session_id"] not in prior and item.get("dialed_addr") == addr
        ]
        return found[-1] if found and ready(found[-1]) else None
    return until(match, "dial " + addr)


def peer_for(item, prior):
    def match():
        for other in enroll(b_session)["sessions"]:
            if other["session_id"] in prior:
                continue
            if other.get("peer_fingerprint") == item["local_fingerprint"] and other.get("local_fingerprint") == item["peer_fingerprint"]:
                return other
        return None
    return until(match, "peer fingerprint bind")


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


def focus_tui(marker, seconds=12, absent=()):
    started = time.monotonic()
    keys = 0
    while time.monotonic() < started + seconds:
        pump(0.2)
        current = terminal()
        if marker in current and all(item not in current for item in absent):
            return current
        if keys < 16 and time.monotonic() - started > 3:
            os.write(master, b"[" if keys < 8 else b"]")
            keys += 1
    raise AssertionError("TUI missing %r\n%s" % (marker, terminal()))


def type_address(address):
    erase = termios.tcgetattr(slave)[6][termios.VERASE]
    if isinstance(erase, int):
        erase = bytes([erase])
    os.write(master, b"m" + erase * 48 + address.encode() + b"\r")
    deadline = time.monotonic() + 6
    while time.monotonic() < deadline:
        pump(0.2)
        if "Esc cancel" not in terminal():
            return
    raise AssertionError(terminal())


def scroll_click(view, bounds):
    x, y, right, bottom = bounds
    if not (y >= 110 and bottom <= 790):
        delta = -260 if bottom > 790 else 260
        view.events([
            {"PointerMoved": [(x + right) / 2, 500]},
            {"MouseWheel": dict(unit="Point", delta=[0, delta], phase="Move", source="Wheel", modifiers=view.modifiers)},
        ])
        time.sleep(0.15)
        return False
    position = [(x + right) / 2, (y + bottom) / 2]
    assert 0 <= position[1] < 800, position
    view.click_at(position)
    return True


def set_address(view, address):
    for _ in range(14):
        field = None
        for _, node in view.tree()[2][0]:
            if node and node[0] == "textInput":
                field = next((value["bounds"] for value in node if isinstance(value, dict) and "bounds" in value), None)
                if field:
                    break
        if not field:
            raise AssertionError("No address field: " + view.text())
        if scroll_click(view, field):
            break
    else:
        raise AssertionError("Address field stayed off screen")
    select_all = dict(view.modifiers, command=True, mac_cmd=sys.platform == "darwin", ctrl=sys.platform != "darwin")
    for pressed in (True, False):
        view.events([{"Key": dict(key="A", physical_key=None, pressed=pressed, repeat=False, modifiers=select_all)}])
    view.events([{"Text": address}])


def click_cancel(view, fingerprint):
    assert fingerprint in view.text()
    for _ in range(40):
        choices = []
        for _, node in view.tree()[2][0]:
            if node[0] == "button" and not node[3] & (1 << 9):
                for value in node:
                    if isinstance(value, dict) and value.get("label") == "Cancel connection":
                        choices.append(value["bounds"])
        # This scenario has one live attempt; earlier cancelled cards retain
        # disabled buttons with the same label and peer fingerprint.
        assert len(choices) <= 1, "More than one live cancellation target"
        if choices and scroll_click(view, choices[0]):
            return
        time.sleep(0.15)
    raise AssertionError("Cancel stayed off screen")


def unchanged(a_base, b_base, a_trust, b_trust):
    assert auth_state(a_session) == a_base, "A authorized_devices changed"
    assert auth_state(b_session) == b_base, "B authorized_devices changed"
    assert trusted(a_session) == a_trust and trusted(b_session) == b_trust


def no_consent(item):
    assert item["local_confirmed"] is False and item["peer_confirmed"] is False
    assert item["state"] != "Succeeded"


def save_pair(name):
    ui.screenshot(out / f"{name}.png")
    out.joinpath(f"{name}.txt").write_text(terminal())


def show_failure(item, name, floor):
    error = item["error"]
    assert error and "\n" not in error
    local = full(item["local_fingerprint"], "local")
    assert not item.get("peer_fingerprint")
    focus_tui(error)
    ui.wait(lambda: ui.text().count("Connection could not be completed") > floor and error in ui.text() and local in ui.text(), "GUI failure", seconds=12)
    assert "Connection could not be completed" in terminal() and local in terminal()
    no_consent(by_id(a_session, item["session_id"]))
    save_pair(name)
    return error


def show_fingerprints(item, peer, floor):
    local, remote = full(item["local_fingerprint"], "local"), full(item["peer_fingerprint"], "peer")
    assert local != remote
    assert peer["local_fingerprint"] == remote and peer["peer_fingerprint"] == local
    no_consent(item)
    no_consent(peer)
    visible = focus_tui(remote, absent=("Connection cancelled", "Connection could not be completed"))
    ui.wait(lambda: ui.text().count("Fingerprints match") > floor and local in ui.text() and remote in ui.text(), "GUI fingerprints", seconds=12)
    assert local in visible and remote in visible and "fingerprints match, confirm" in visible
    return local, remote


try:
    if not select.select([b_proc.stdout], [], [], 12)[0]:
        raise AssertionError("Peer fixture did not start; see peer-fixture.log")
    b_session = json.loads(b_proc.stdout.readline())
    a_base, b_base = auth_state(a_session), auth_state(b_session)
    a_trust, b_trust = trusted(a_session), trusted(b_session)
    opened = control(b_session, "EnrollmentOpen")["enrollment"]
    listen = opened.get("listen_addr") or ""
    assert listen.startswith("127.0.0.1:"), listen
    ui = Inspection(int(os.environ.get("CLIPSYNC_UI_PORT", "18762")))
    master, slave = pty.openpty()
    before = termios.tcgetattr(slave)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
    tui = subprocess.Popen(
        [str(binary), "--config", a_session["config"], "tui"],
        stdin=slave, stdout=slave, stderr=slave, start_new_session=True,
        env=os.environ | {"TERM": "xterm-256color", "NO_COLOR": "1"},
    )
    until(lambda: "Ready to sync" in terminal() or "Your clipboard, everywhere" in terminal(), "TUI ready")
    os.write(master, b"2")
    pump(0.5)
    os.write(master, b"e")
    ui.click("Devices")
    ui.wait(lambda: ui.enabled("Allow a new device"), "enrollment action ready", seconds=12)
    ui.click("Allow a new device")
    ui.wait(lambda: "Ready for a new device" in ui.text(), "enrollment open", seconds=12)
    focus_tui("Allowing connections")

    closed_tui = released_loopback()
    peer_before = ids(b_session)
    prior = ids(a_session)
    fail_cards = ui.text().count("Connection could not be completed")
    type_address(closed_tui)
    failed = await_dial(closed_tui, prior, lambda item: item["state"] == "Failed" and item.get("error"))
    assert ids(b_session) == peer_before, "unreachable dial reached the peer"
    recorded["tui_unreachable_error"] = show_failure(failed, "unreachable-tui", fail_cards)
    unchanged(a_base, b_base, a_trust, b_trust)

    prior = ids(a_session)
    peer_before = ids(b_session)
    confirm_cards = ui.text().count("Fingerprints match")
    type_address(listen)
    pending = await_dial(listen, prior, lambda item: item["state"] == "AwaitingLocalConfirm" and item.get("peer_fingerprint"))
    bound = peer_for(pending, peer_before)
    local_fp, peer_fp = show_fingerprints(pending, bound, confirm_cards)
    os.write(master, b"c")
    until(lambda: by_id(a_session, pending["session_id"])["state"] == "Cancelled", "TUI cancel")
    cancelled = by_id(a_session, pending["session_id"])
    no_consent(cancelled)
    focus_tui("Connection cancelled")
    ui.wait(lambda: "Connection cancelled" in ui.text() and peer_fp in ui.text(), "GUI sees TUI cancel", seconds=12)
    no_consent(by_id(b_session, bound["session_id"]))
    save_pair("cancel-tui")
    unchanged(a_base, b_base, a_trust, b_trust)
    recorded["tui_cancel"] = {"local": local_fp, "peer": peer_fp, "state": cancelled["state"]}

    closed_gui = released_loopback()
    peer_before = ids(b_session)
    prior = ids(a_session)
    fail_cards = ui.text().count("Connection could not be completed")
    set_address(ui, closed_gui)
    ui.wait(lambda: ui.enabled("Connect to address"), "GUI dial ready")
    ui.click("Connect to address")
    failed = await_dial(closed_gui, prior, lambda item: item["state"] == "Failed" and item.get("error"))
    assert ids(b_session) == peer_before, "unreachable GUI dial reached the peer"
    recorded["gui_unreachable_error"] = show_failure(failed, "unreachable-gui", fail_cards)
    unchanged(a_base, b_base, a_trust, b_trust)

    prior = ids(a_session)
    peer_before = ids(b_session)
    confirm_cards = ui.text().count("Fingerprints match")
    set_address(ui, listen)
    ui.wait(lambda: ui.enabled("Connect to address"), "GUI retry ready")
    ui.click("Connect to address")
    pending = await_dial(listen, prior, lambda item: item["state"] == "AwaitingLocalConfirm" and item.get("peer_fingerprint"))
    bound = peer_for(pending, peer_before)
    local_fp, peer_fp = show_fingerprints(pending, bound, confirm_cards)
    click_cancel(ui, peer_fp)
    until(lambda: by_id(a_session, pending["session_id"])["state"] == "Cancelled", "GUI cancel")
    cancelled = by_id(a_session, pending["session_id"])
    no_consent(cancelled)
    focus_tui("Connection cancelled")
    ui.wait(lambda: "Connection cancelled" in ui.text() and peer_fp in ui.text(), "GUI cancel visible", seconds=12)
    no_consent(by_id(b_session, bound["session_id"]))
    save_pair("cancel-gui")
    unchanged(a_base, b_base, a_trust, b_trust)
    recorded["gui_cancel"] = {"local": local_fp, "peer": peer_fp, "state": cancelled["state"]}

    os.write(master, b"q")
    assert tui.wait(timeout=5) == 0
    pump(0.6)
    assert termios.tcgetattr(slave) == before, "termios not restored"
    assert b"\x1b[?1049l" in raw, "alternate screen not left"
    assert b"\x1b[?25h" in raw, "cursor not restored"
    result = {
        "passed": [
            "TUI dial of a released loopback port fails with the inspect error and writes no trust",
            "TUI cancel before confirm shows both full fingerprints and writes no trust",
            "GUI dial of a distinct released loopback port fails in both clients and writes no trust",
            "GUI cancel before confirm shows both full fingerprints and writes no trust",
            "neither side confirms; both authorized_devices files stay byte-identical",
            "terminal modes, cursor, and alternate screen restored",
        ],
        "closed_ports": [closed_tui, closed_gui],
        "peer_listen": listen,
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
    if b_proc.poll() is None:
        b_proc.send_signal(signal.SIGINT)
        try:
            b_proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            b_proc.kill()
            b_proc.wait()
    if a_session:
        try:
            control(a_session, {"EnrollmentCancel": {"session_id": None}})
        except Exception:
            pass
    if ui:
        ui.socket.close()
    if master is not None:
        os.close(master)
    if slave is not None:
        os.close(slave)
    peer_log.close()
