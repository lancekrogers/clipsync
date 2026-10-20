"""Actual wrong-UID IPC check, only in the root disposable Linux fixture."""
import json
import os
import stat
import subprocess
import sys
from pathlib import Path

from control_client import request

assert sys.platform == "linux" and os.geteuid() == 0
assert os.environ.get("CLIPSYNC_DISPOSABLE_DESKTOP") == "1"
session = json.loads(Path(os.environ["CLIPSYNC_UI_SESSION"]).read_text())
path = Path(session["socket"])
runtime = Path(os.environ["XDG_RUNTIME_DIR"])
assert runtime.parent == Path("/tmp") and runtime.name.startswith("clipsync-runtime.")
assert path.parent.parent == runtime
runtime_mode = stat.S_IMODE(runtime.stat().st_mode)
directory_mode = stat.S_IMODE(path.parent.stat().st_mode)
socket_mode = stat.S_IMODE(path.stat().st_mode)
assert directory_mode == 0o700 and socket_mode == 0o600
assert request(session, "Status")["status"] is not None
try:
    # Bypass only the fixture's filesystem protection, to isolate peer_cred.
    runtime.chmod(0o711)
    path.parent.chmod(0o755)
    path.chmod(0o666)
    child = subprocess.run(
        ["runuser", "-u", "nobody", "--", sys.executable, "-c", """
import json, os, socket, struct, sys
assert os.geteuid() != 0
with socket.socket(socket.AF_UNIX) as stream:
    stream.settimeout(5)
    stream.connect(sys.argv[1])  # PermissionError here is NOT a passing test.
    payload = json.dumps({'version':1, 'command':'Status'}).encode()
    try:
        stream.sendall(struct.pack('>I',len(payload))+payload)
        data=stream.recv(4)
    except (ConnectionResetError, BrokenPipeError):
        data=b''
    assert not data, 'Daemon answered a wrong-user client'
print('connected_then_rejected')
""", str(path)],
        capture_output=True, text=True, timeout=10,
    )
    assert child.returncode == 0, child.stderr
    assert child.stdout.strip() == "connected_then_rejected", child.stdout
finally:
    path.chmod(socket_mode)
    path.parent.chmod(directory_mode)
    runtime.chmod(runtime_mode)
assert request(session, "Status")["status"] is not None
out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)
result = {"passed": [
    "wrong UID connects past relaxed fixture permissions but receives no response",
    "same-user IPC still works after rejection",
    "fixture socket permissions restored to 0700/0600",
]}
out.joinpath("results.json").write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps(result))
