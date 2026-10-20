"""Native acceptance runner. This never opens the general NSPasteboard."""
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
    raise SystemExit('Use companion-test-linux for the disposable Linux desktop.')
root = Path(__file__).resolve().parents[2]
out = root/'target/companion-evidence/macos'
out.mkdir(parents=True, exist_ok=True)
for port in (18762,18763,18764,18765):
    with socket.socket() as probe:
        try: probe.bind(('127.0.0.1',port))
        except OSError: raise SystemExit(f'Test port {port} is in use; close the previous test fixture first.')
env = os.environ | {'CLIPSYNC_TEST_PASTEBOARD':'org.clipsync.test.' + str(uuid.uuid4())}
fixture = subprocess.Popen([str(root/'target/debug/examples/companion_fixture')], env=env, stdout=subprocess.PIPE, stderr=out.joinpath('fixture.log').open('w'), text=True)
app = None
try:
    if not select.select([fixture.stdout],[],[],12)[0]:
        raise RuntimeError('Isolated fixture did not start; see fixture.log')
    session = json.loads(fixture.stdout.readline())
    session['pasteboard'] = env['CLIPSYNC_TEST_PASTEBOARD']
    session_path = out/'session.json';session_path.write_text(json.dumps(session))
    env.update(CLIPSYNC_UI_SESSION=str(session_path), EGUI_INSPECTION='127.0.0.1:18762')
    app = subprocess.Popen([str(root/'target/debug/clipsync-desktop'),'--config',session['config'],'--no-tray'], env=env, stdout=out.joinpath('app.log').open('w'), stderr=subprocess.STDOUT)
    for script,args in [
        ('companion_ui.py',[str(out/'gui')]),
        ('companion_pty.py',[str(root/'target/debug/clipsync'),str(out/'tui')]),
        ('companion_recovery.py',[str(root/'target/debug/clipsync'),str(out/'recovery')]),
        ('companion_large_history.py',[str(out/'large-history')]),
        ('companion_enrollment_ui.py',[str(out/'enrollment')]),
        ('companion_tray.py',[str(out/'tray')]),
        ('companion_setup_ui.py',[str(out/'setup')]),
    ]:
        if script == 'companion_tray.py':
            # System Events resolves same-name process references ambiguously.
            app.send_signal(signal.SIGINT);app.wait(timeout=5)
        subprocess.run([sys.executable,str(root/'scripts/test'/script),*args],env=env,check=True,timeout=180)
    print(f'GUI, PTY and enrollment evidence: {out}')
finally:
    for process in (app,fixture):
        if process and process.poll() is None:
            process.send_signal(signal.SIGINT)
            try: process.wait(timeout=5)
            except subprocess.TimeoutExpired: process.kill();process.wait()
