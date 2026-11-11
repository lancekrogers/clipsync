"""Two isolated real windows/daemons pair with a code, and persisted trust.
Start fixture/app A separately; this harness starts B with another private pasteboard.
B allows a device; A types the code B shows. Nothing is confirmed by hand.
"""
import json
import os
import select
import signal
import subprocess
import sys
import uuid
from pathlib import Path
from inspection_client import Inspection
from control_client import request as control

root = Path(__file__).resolve().parents[2]
out = Path(sys.argv[1]); out.mkdir(parents=True, exist_ok=True)
a_session = json.loads(Path(os.environ['CLIPSYNC_UI_SESSION']).read_text())
if sys.platform == 'darwin':
    assert a_session['pasteboard'].startswith('org.clipsync.test.')
else:
    assert os.environ.get('CLIPSYNC_DISPOSABLE_DESKTOP') == '1'
b_env = os.environ | {'CLIPSYNC_TEST_PASTEBOARD': 'org.clipsync.test.' + str(uuid.uuid4()), 'CLIPSYNC_FIXTURE_NAME': 'Peer test Mac'}
b_fixture = subprocess.Popen([str(root / 'target/debug/examples/companion_fixture')], env=b_env, stdout=subprocess.PIPE, stderr=out.joinpath('peer-fixture.log').open('w'), text=True)
b_app = None


def set_field(view, index, text):
    """Type into the index-th text field (0 = code, 1 = address)."""
    view.type_into(index, text)


try:
    assert select.select([b_fixture.stdout], [], [], 12)[0], 'Peer fixture did not start'
    b_session = json.loads(b_fixture.stdout.readline())
    for session in (a_session, b_session):
        assert 'enrollment_code' in control(session, 'Status')['features'], 'binaries predate code pairing'
    b_app = subprocess.Popen([str(root / 'target/debug/clipsync-desktop'), '--config', b_session['config'], '--no-tray'], env=b_env | {'EGUI_INSPECTION': '127.0.0.1:18763'}, stdout=out.joinpath('peer-app.log').open('w'), stderr=subprocess.STDOUT)
    a, b = Inspection(18762), Inspection(18763)
    for ui, name in ((a, 'A'), (b, 'B')):
        ui.click('Devices')
        ui.wait(lambda ui=ui: ui.enabled('Allow a new device'), f'{name} pairing ready')
    b.click('Allow a new device')
    b.wait(lambda: 'Pairing code' in b.text(), 'B shows a code')
    snapshot = control(b_session, {'EnrollmentInspect': {'session_id': None}})['enrollment']
    code = snapshot['code']['code']
    # The window shows the same code the daemon will accept, grouped for reading.
    assert f'{code[:3]} {code[3:]}' in b.text()
    b.screenshot(out/'code-b.png')
    assert not control(a_session, 'TrustedDevices')['trusted_devices']
    assert not control(b_session, 'TrustedDevices')['trusted_devices']
    set_field(a, 0, code)
    set_field(a, 1, snapshot['listen_addr'])
    a.wait(lambda: a.enabled('Pair with address'), 'A pair ready')
    a.click('Pair with address')
    a.wait(lambda: len(control(a_session, 'TrustedDevices')['trusted_devices']) == 1, 'A saved B', seconds=15)
    b.wait(lambda: len(control(b_session, 'TrustedDevices')['trusted_devices']) == 1, 'B saved A', seconds=15)
    for ui, session in ((a, a_session), (b, b_session)):
        ui.wait(lambda session=session: any(item['state'] == 'Succeeded' for item in control(session, {'EnrollmentInspect': {'session_id': None}})['enrollment']['sessions']), 'both peers acknowledge durable success')
    # Independently inspect the durable files, not merely controller state.
    for session in (a_session, b_session):
        auth = Path(session['config']).parent/'authorized_devices'
        assert auth.read_text().count('ssh-ed25519 ') == 1
        assert auth.stat().st_mode & 0o077 == 0
    for ui in (a, b):
        ui.wait(lambda ui=ui: 'Paired. Both computers saved the pairing.' in ui.text(), 'success visible in both native windows')
    a.screenshot(out/'paired-a.png'); b.screenshot(out/'paired-b.png')
    assert not control(b_session, {'EnrollmentInspect': {'session_id': None}})['enrollment']['listening'], 'B keeps allowing after pairing'
    for ui, session in ((a, a_session), (b, b_session)):
        ui.wait(lambda ui=ui: 'Remove authorization' in ui.text(), 'authorized device visible')
        ui.click('Remove authorization…'); ui.click('Remove this device')
        ui.wait(lambda session=session: not control(session, 'TrustedDevices')['trusted_devices'], 'revocation persists')
        assert 'ssh-ed25519 ' not in (Path(session['config']).parent/'authorized_devices').read_text()
    result = {'passed': ['two real native pairing screens', 'B shows the code its daemon accepts', 'A types code and LAN address in the GUI', 'no confirm step; both files gain exactly one key, mode 0600', 'both peers reach acknowledged success', 'allowing window closes after pairing', 'native explicit revocation removes both durable pins']}
    out.joinpath('results.json').write_text(json.dumps(result, indent=2) + '\n'); print(json.dumps(result))
finally:
    control(a_session, {'EnrollmentCancel': {'session_id': None}})
    for process in (b_app, b_fixture):
        if process and process.poll() is None:
            process.send_signal(signal.SIGINT)
            try: process.wait(timeout=5)
            except subprocess.TimeoutExpired: process.kill(); process.wait()
