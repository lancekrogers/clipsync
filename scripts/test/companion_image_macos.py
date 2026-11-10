"""Isolated macOS GUI image-history regression; never opens the general pasteboard."""
import json
import os
import select
import signal
import socket
import subprocess
import sys
import uuid
from pathlib import Path

if sys.platform != 'darwin':
    raise SystemExit('This fixture requires macOS.')
root = Path(__file__).resolve().parents[2]
out = root / 'target/companion-evidence/image-macos'
out.mkdir(parents=True, exist_ok=True)
port = 18796
with socket.socket() as probe:
    try:
        probe.bind(('127.0.0.1', port))
    except OSError:
        raise SystemExit(f'Inspection port {port} is in use.')
env = os.environ | {
    'CLIPSYNC_TEST_PASTEBOARD': 'org.clipsync.test.' + str(uuid.uuid4()),
    'CLIPSYNC_IMAGE_FIXTURE': '1',
    'CLIPSYNC_UI_PORT': str(port),
}
fixture_log = out.joinpath('fixture.log').open('w')
app_log = out.joinpath('app.log').open('w')
fixture = subprocess.Popen([str(root / 'target/debug/examples/companion_fixture')], env=env,
                           stdout=subprocess.PIPE, stderr=fixture_log, text=True)
app = None
try:
    if not select.select([fixture.stdout], [], [], 12)[0]:
        raise RuntimeError('Isolated fixture did not start; see fixture.log')
    session = json.loads(fixture.stdout.readline())
    session['pasteboard'] = env['CLIPSYNC_TEST_PASTEBOARD']
    session_path = out / 'session.json'
    session_path.write_text(json.dumps(session))
    env.update(CLIPSYNC_UI_SESSION=str(session_path), EGUI_INSPECTION=f'127.0.0.1:{port}')
    app = subprocess.Popen([str(root / 'target/debug/clipsync-desktop'), '--config', session['config'], '--no-tray'],
                           env=env, stdout=app_log, stderr=subprocess.STDOUT)
    subprocess.run([sys.executable, str(root / 'scripts/test/companion_ui.py'), str(out / 'gui')],
                   env=env, check=True, timeout=180)
    print(f'Isolated image GUI evidence: {out}')
finally:
    for process in (app, fixture):
        if process and process.poll() is None:
            process.send_signal(signal.SIGINT)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
    fixture_log.close()
    app_log.close()
