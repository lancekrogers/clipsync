"""Launch the actual release bundle against a private fixture; never install it."""
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

assert sys.platform=='darwin'
root=Path(__file__).resolve().parents[2]
bundle=Path(sys.argv[1]).resolve()
out=Path(sys.argv[2]);out.mkdir(parents=True,exist_ok=True)
cli=bundle/'Contents/MacOS/clipsync'
gui=bundle/'Contents/MacOS/clipsync-desktop'
assert 'tui' in subprocess.check_output([str(cli),'--help'],text=True)
version=subprocess.check_output([str(cli),'version'],text=True)
assert 'Profile: release' in version
out.joinpath('version.txt').write_text(version)
env=os.environ|{'CLIPSYNC_TEST_PASTEBOARD':'org.clipsync.test.'+str(uuid.uuid4())}
fixture=subprocess.Popen([str(root/'target/debug/examples/companion_fixture')],env=env,stdout=subprocess.PIPE,stderr=out.joinpath('fixture.log').open('w'),text=True)
app=None
try:
    assert select.select([fixture.stdout],[],[],15)[0], 'fixture startup timeout'
    session=json.loads(fixture.stdout.readline())
    app=subprocess.Popen([str(gui),'--config',session['config'],'--no-tray'],env=env|{'EGUI_INSPECTION':'127.0.0.1:18766'},stdout=out.joinpath('app.log').open('w'),stderr=subprocess.STDOUT)
    deadline=time.monotonic()+30
    while time.monotonic()<deadline:
        assert app.poll() is None, 'release app exited; see app.log'
        script=f'tell application "System Events" to tell (first application process whose unix id is {app.pid}) to get count windows'
        answer=subprocess.run(['osascript','-e',script],text=True,capture_output=True,timeout=10)
        if answer.returncode==0 and int(answer.stdout.strip())>0:break
        time.sleep(.2)
    else:raise AssertionError('release bundle window never opened')
    time.sleep(1)
    with socket.socket() as probe:
        probe.settimeout(1)
        assert probe.connect_ex(('127.0.0.1',18766)) != 0, 'release bundle exposes test inspection'
    assert request(session,'Status')['pid']==fixture.pid
    app.send_signal(signal.SIGINT);app.wait(timeout=5)
    assert request(session,'Status')['pid']==fixture.pid
    result={'passed':['release CLI includes TUI and prints build metadata before setup','packaged native window opens with explicit private config','production GUI has no UI inspection listener','closing bundle leaves fixture daemon running'],'public_release_ready':False}
    out.joinpath('results.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
finally:
    for process in (app,fixture):
        if process and process.poll() is None:
            process.send_signal(signal.SIGINT)
            try:process.wait(timeout=5)
            except subprocess.TimeoutExpired:process.kill();process.wait()
