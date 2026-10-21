"""Per-config desktop single instance against a private companion_fixture.

Launches its own fixture. Does not use personal clipboard, config, or services.
Inspection commands are only those in inspection_client.py.
"""
import json
import os
import select
import signal
import socket
import subprocess
import sys
import time
import uuid
from pathlib import Path

from control_client import request
from inspection_client import Inspection

root = Path(__file__).resolve().parents[2]
out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)
desktop = root / "target" / "debug" / "clipsync-desktop"
fixture_bin = root / "target" / "debug" / "examples" / "companion_fixture"
pasteboard = "org.clipsync.test." + str(uuid.uuid4())
if sys.platform == "darwin":
    env = os.environ | {"CLIPSYNC_TEST_PASTEBOARD": pasteboard}
elif sys.platform == "linux":
    assert os.environ.get("CLIPSYNC_DISPOSABLE_DESKTOP") == "1"
    env = os.environ.copy()
else:
    raise SystemExit(f"unsupported platform {sys.platform}")

passed = []
limitations = [
    "No Close or Quit command exists on the pinned inspection client (GetTree, ApplyEvents, GetScreenshot). "
    "The no-tray app is stopped with SIGINT. That is process cleanup, not a native close/quit acceptance claim.",
    "Window raise checks count windows on the scoped unix id. Frontmost/key focus on the compositor is not claimed.",
]


def port_open(port):
    try:
        with socket.create_connection(("127.0.0.1", port), 0.2):
            return True
    except OSError:
        return False


for port in (18766, 18767):
    assert not port_open(port), f"{port} is already in use"

fixture = subprocess.Popen(
    [str(fixture_bin)],
    env=env,
    stdout=subprocess.PIPE,
    stderr=out.joinpath("fixture.log").open("w"),
    text=True,
)
apps = []


def stop(process, sig=signal.SIGINT):
    if process is None or process.poll() is not None:
        return
    process.send_signal(sig)
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def daemon_pid(session):
    return request(session, "Status")["pid"]


def launch(session, config, port, label, no_tray=True, overrides=None):
    child_env = env | (overrides or {})
    child_env.pop("EGUI_INSPECTION", None)
    if port is not None:
        child_env["EGUI_INSPECTION"] = f"127.0.0.1:{port}"
    command = [str(desktop), "--config", str(config)]
    if no_tray:
        command.append("--no-tray")
    log = out.joinpath(f"{label}.log").open("w")
    proc = subprocess.Popen(command, env=child_env, stdout=log, stderr=subprocess.STDOUT)
    apps.append(proc)
    return proc


def windows(pid):
    script = f'''tell application "System Events"
set targetProcess to first application process whose unix id is {pid}
tell targetProcess
count windows
end tell
end tell'''
    raw = subprocess.check_output(["osascript", "-e", script], text=True, timeout=12).strip()
    return int(raw)


def assert_same_config_rejected(session, config, label, overrides=None):
    before = daemon_pid(session)
    proc = launch(session, config, 18767, label, overrides=overrides)
    saw_window = False
    deadline = time.monotonic() + 20
    while proc.poll() is None and time.monotonic() < deadline:
        assert not port_open(18767), f"{label} bound an inspection listener"
        if sys.platform == "darwin":
            try:
                if windows(proc.pid) > 0:
                    saw_window = True
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired, ValueError):
                pass
        time.sleep(0.15)
    code = proc.wait(timeout=5) if proc.poll() is None else proc.returncode
    assert code == 0, f"{label} exited {code}"
    assert not saw_window, f"{label} created a window"
    assert not port_open(18767)
    assert daemon_pid(session) == before
    return before


try:
    assert select.select([fixture.stdout], [], [], 12)[0], "fixture did not start"
    session = json.loads(fixture.stdout.readline())
    config = Path(session["config"])
    assert config.is_file()
    assert pasteboard.startswith("org.clipsync.test.")
    primary = launch(session, config, 18766, "app-a")
    ui = Inspection(18766)
    original = daemon_pid(session)
    assert original == session["pid"]
    assert_same_config_rejected(session, config, "duplicate")
    assert primary.poll() is None
    ui.wait(lambda: "Ready to sync" in ui.text(), "first app sees the daemon")
    ui.wait(lambda: daemon_pid(session) == original, "daemon pid changed after duplicate")
    passed.append("same config second process exits 0 with no window or listener; first stays up; daemon pid unchanged")

    ui.click("History")
    ui.wait(lambda: "Clipboard history" in ui.text(), "history route")
    out.joinpath("history.txt").write_text(ui.text())
    ui.screenshot(out / "history.png")
    assert_same_config_rejected(session, config, "duplicate-on-history")
    assert primary.poll() is None
    ui.wait(lambda: "Clipboard history" in ui.text(), "history route survived duplicate")
    out.joinpath("history-after-duplicate.txt").write_text(ui.text())
    ui.screenshot(out / "history-after-duplicate.png")
    passed.append("history route stays on the first app when a same-config duplicate exits")

    alternate_runtime = config.parent / "alternate-runtime"
    alternate_runtime.mkdir(mode=0o700)
    assert_same_config_rejected(session, config, "different-runtime-environment", {
        "XDG_RUNTIME_DIR": str(alternate_runtime), "TMPDIR": str(alternate_runtime),
    })
    assert primary.poll() is None
    passed.append("different launcher runtime environment still activates the same owner")

    link = config.parent / "config-link.toml"
    link.symlink_to(config.name)
    assert_same_config_rejected(session, link, "symlink-alias")
    assert primary.poll() is None
    ui.wait(lambda: "Clipboard history" in ui.text(), "history route survived symlink duplicate")
    passed.append("symlink to the same config is treated as the same instance")

    other = config.parent / "config-other.toml"
    other.write_bytes(config.read_bytes())
    second = launch(session, other, 18767, "app-b")
    other_ui = Inspection(18767)
    assert second.poll() is None
    assert primary.poll() is None
    other_ui.wait(lambda: "Ready to sync" in other_ui.text(), "independent app sees the daemon")
    out.joinpath("second-config.txt").write_text(other_ui.text())
    other_ui.screenshot(out / "second-config.png")
    assert daemon_pid(session) == original
    stop(second)
    assert second.poll() is not None
    assert not port_open(18767)
    assert primary.poll() is None
    assert daemon_pid(session) == original
    ui.wait(lambda: "Clipboard history" in ui.text(), "first app died when the other config closed")
    passed.append("copied config in the fixture directory is an independent GUI on the same daemon")

    primary.send_signal(signal.SIGKILL)
    primary.wait(timeout=5)
    deadline = time.monotonic() + 5
    while port_open(18766) and time.monotonic() < deadline:
        time.sleep(0.15)
    assert not port_open(18766), "inspection port stayed bound after SIGKILL"
    relaunch = launch(session, config, 18766, "app-relaunch")
    ui = Inspection(18766)
    ui.wait(lambda: "Ready to sync" in ui.text(), "relaunched app sees the daemon")
    assert daemon_pid(session) == original
    out.joinpath("relaunch.txt").write_text(ui.text())
    ui.screenshot(out / "relaunch.png")
    passed.append("SIGKILL of the app then same-config relaunch recovers a stale instance lock; daemon pid unchanged")

    stop(relaunch)
    assert relaunch.returncode in (0, -signal.SIGINT)
    assert daemon_pid(session) == original
    assert fixture.poll() is None
    passed.append("no-tray process stopped with SIGINT; daemon still serves Status")

    if sys.platform == "darwin":
        enabled = subprocess.check_output(
            ["osascript", "-e", 'tell application "System Events" to get UI elements enabled'],
            text=True,
            timeout=12,
        ).strip()
        assert enabled == "true", "Accessibility is not enabled; hidden-window check not run."
        tray = launch(session, config, None, "app-tray", no_tray=False)
        deadline = time.monotonic() + 30
        opened = False
        while time.monotonic() < deadline:
            assert tray.poll() is None, f"tray app exited {tray.returncode}"
            try:
                if windows(tray.pid) > 0:
                    opened = True
                    break
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired, ValueError):
                pass
            time.sleep(0.2)
        assert opened, "tray app window did not open"
        close = f'''tell application "System Events"
set targetProcess to first application process whose unix id is {tray.pid}
tell targetProcess
perform action "AXPress" of (first button of window 1 whose subrole is "AXCloseButton")
end tell
end tell'''
        subprocess.check_call(["osascript", "-e", close], timeout=12)
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            assert tray.poll() is None
            if windows(tray.pid) == 0:
                break
            time.sleep(0.2)
        else:
            raise AssertionError("red close did not hide the tray app window")
        raiser = launch(session, config, None, "app-raise")
        deadline = time.monotonic() + 20
        raised = False
        while time.monotonic() < deadline:
            assert tray.poll() is None, "scoped tray app exited during raise"
            try:
                if windows(tray.pid) > 0:
                    raised = True
                    break
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired, ValueError):
                pass
            time.sleep(0.15)
        if raiser.poll() is None:
            raiser.wait(timeout=8)
        assert raiser.returncode == 0
        assert raised, "same-config launch did not raise the hidden tray window"
        assert windows(tray.pid) > 0
        assert daemon_pid(session) == original
        passed.append("red close hides the scoped tray app; a same-config launch raises that unix id")
    else:
        limitations.append("Native hidden-window activation is macOS System Events only and was not run.")

    result = {"passed": passed, "limitations": limitations, "daemon_pid": original}
    out.joinpath("results.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))
finally:
    for proc in reversed(apps):
        stop(proc)
    stop(fixture)
