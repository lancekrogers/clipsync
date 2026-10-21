"""Two isolated real windows/daemons, mutual confirmation and persisted trust.
Start fixture/app A separately; this harness starts B with another private pasteboard.
"""
import json
import os
import select
import signal
import socket
import struct
import subprocess
import sys
import time
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
b_env = os.environ | {'CLIPSYNC_TEST_PASTEBOARD': 'org.clipsync.test.' + str(uuid.uuid4())}
b_fixture = subprocess.Popen([str(root / 'target/debug/examples/companion_fixture')], env=b_env, stdout=subprocess.PIPE, stderr=out.joinpath('peer-fixture.log').open('w'), text=True)
b_app = None


try:
    assert select.select([b_fixture.stdout], [], [], 12)[0], 'Peer fixture did not start'
    b_session = json.loads(b_fixture.stdout.readline())
    b_app = subprocess.Popen([str(root / 'target/debug/clipsync-desktop'), '--config', b_session['config'], '--no-tray'], env=b_env | {'EGUI_INSPECTION': '127.0.0.1:18763'}, stdout=out.joinpath('peer-app.log').open('w'), stderr=subprocess.STDOUT)
    a, b = Inspection(18762), Inspection(18763)
    for ui, name in ((a, 'A'), (b, 'B')):
        ui.click('Devices')
        ui.wait(lambda ui=ui: ui.enabled('Allow a new device'), 'enrollment action ready')
        ui.click('Allow a new device')
        ui.wait(lambda ui=ui: 'Ready for a new device' in ui.text(), f'{name} enrollment open')
    snapshot = control(b_session, {'EnrollmentInspect': {'session_id': None}})['enrollment']
    # Enter the peer's real bound loopback address through the native UI.
    for _, node in a.tree()[2][0]:
        if node[0] == 'textInput':
            bounds = next((v['bounds'] for v in node if isinstance(v, dict) and 'bounds' in v), None)
            if bounds:
                x,y,r,d = bounds; position=[(x+r)/2,(y+d)/2]
                a.click_at(position)
                a.events([{'Text':snapshot['listen_addr']}]);break
    else: raise AssertionError('No address field')
    a.click('Connect to address')
    for ui in (a,b): ui.wait(lambda ui=ui: 'Fingerprints match' in ui.text(), 'verification appears')
    sa = control(a_session, {'EnrollmentInspect': {'session_id': None}})['enrollment']['sessions'][-1]
    sb = control(b_session, {'EnrollmentInspect': {'session_id': None}})['enrollment']['sessions'][-1]
    assert sa['local_fingerprint'] == sb['peer_fingerprint']
    assert sb['local_fingerprint'] == sa['peer_fingerprint']
    for ui in (a,b):
        assert sa['local_fingerprint'] in ui.text() and sb['local_fingerprint'] in ui.text()
    a.screenshot(out/'verify-a.png');b.screenshot(out/'verify-b.png')
    a.click('Fingerprints match — confirm')
    a.wait(lambda: 'Confirmed here; waiting for both computers.' in a.text(), 'one-sided confirmation stays pending')
    assert not control(a_session, 'TrustedDevices')['trusted_devices']
    assert not control(b_session, 'TrustedDevices')['trusted_devices']
    b.click('Fingerprints match — confirm')
    a.wait(lambda: len(control(a_session, 'TrustedDevices')['trusted_devices']) == 1, 'A persists after mutual confirm')
    b.wait(lambda: len(control(b_session, 'TrustedDevices')['trusted_devices']) == 1, 'B persists after mutual confirm')
    for ui,session in ((a,a_session),(b,b_session)):
        ui.wait(lambda session=session: all(item['state'] == 'Succeeded' for item in control(session, {'EnrollmentInspect': {'session_id': None}})['enrollment']['sessions']), 'both peers acknowledge durable success')
    # Independently inspect the durable files, not merely controller state.
    for session in (a_session,b_session):
        auth = Path(session['config']).parent/'authorized_devices'
        assert 'ssh-ed25519 ' in auth.read_text()
        assert auth.stat().st_mode & 0o077 == 0
    for ui in (a,b): ui.wait(lambda ui=ui: 'Device authorized' in ui.text(), 'success visible in both native windows')
    a.screenshot(out/'paired-a.png');b.screenshot(out/'paired-b.png')
    for ui,session in ((a,a_session),(b,b_session)):
        ui.wait(lambda ui=ui: 'Remove authorization' in ui.text(), 'authorized device visible')
        ui.click('Remove authorization…');ui.click('Remove this device')
        ui.wait(lambda session=session: not control(session, 'TrustedDevices')['trusted_devices'], 'revocation persists')
        assert 'ssh-ed25519 ' not in (Path(session['config']).parent/'authorized_devices').read_text()
    result={'passed':['two real native enrollment screens','manual LAN address via GUI','TLS-bound full fingerprints match both screens','one confirmation stays visibly pending and writes no authorization','both confirmations persist separate private trust files','both peers reach acknowledged success','native explicit revocation removes both durable pins']}
    out.joinpath('results.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
finally:
    control(a_session, {'EnrollmentCancel': {'session_id': None}})
    for process in (b_app,b_fixture):
        if process and process.poll() is None:
            process.send_signal(signal.SIGINT)
            try: process.wait(timeout=5)
            except subprocess.TimeoutExpired: process.kill();process.wait()
