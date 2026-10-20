"""Real macOS close, tray pause/reopen/quit; uses the existing isolated daemon.
Requires System Events accessibility already enabled. Never requests permissions.
"""
import json
import os
import signal
import shutil
import subprocess
import sys
import time
from pathlib import Path
from control_client import request

assert sys.platform == 'darwin'
assert shutil.which('cliclick'), 'Native tray acceptance requires cliclick (brew install cliclick).'
root = Path(__file__).resolve().parents[2]
session = json.loads(Path(os.environ['CLIPSYNC_UI_SESSION']).read_text())
assert session['pasteboard'].startswith('org.clipsync.test.')
out = Path(sys.argv[1]);out.mkdir(parents=True, exist_ok=True)


def apple(script):
    return subprocess.check_output(['osascript','-e',script],text=True,timeout=12).strip()


assert apple('tell application "System Events" to get UI elements enabled') == 'true', 'Accessibility is not enabled; tray check not run.'
app = subprocess.Popen([str(root/'target/debug/clipsync-desktop'),'--config',session['config']],env=os.environ|{'EGUI_INSPECTION':'127.0.0.1:18764'},stdout=out.joinpath('app.log').open('w'),stderr=subprocess.STDOUT)


def ui(script):
    return apple(f'''tell application "System Events"
set targetProcess to first application process whose unix id is {app.pid}
tell targetProcess
{script}
end tell
end tell''')


def wait(predicate, message):
    deadline=time.monotonic()+30
    while time.monotonic()<deadline:
        try:
            if predicate():return
        except subprocess.CalledProcessError:
            assert app.poll() is None, f"App exited {app.returncode}; see app.log"
        time.sleep(.2)
    raise AssertionError(message)


def menu(item):
    position = ui('get position of menu bar item 1 of menu bar 2').split(',')
    x,y = (int(value.strip()) for value in position)
    subprocess.run(['cliclick', f'c:={x+16},={y+12}'],check=True,timeout=5)
    wait(lambda: ui('exists menu 1 of menu bar item 1 of menu bar 2') == 'true', 'native tray menu did not open')
    # Item names are fixed application labels, not peer input.
    ui(f'click menu item "{item}" of menu 1 of menu bar item 1 of menu bar 2')

try:
    wait(lambda: int(ui('count windows')) > 0, 'native window did not open')
    wait(lambda: ui('exists menu bar item 1 of menu bar 2') == 'true', 'menu-bar icon not available')
    ui('perform action "AXPress" of (first button of window 1 whose subrole is "AXCloseButton")')
    wait(lambda: int(ui('count windows')) == 0, 'red close should hide the window')
    assert app.poll() is None
    assert not request(session,'Status')['status']['paused']
    menu('Pause / resume syncing')
    wait(lambda: request(session,'Status')['status']['paused'], 'tray pause while hidden')
    menu('Pause / resume syncing')
    wait(lambda: not request(session,'Status')['status']['paused'], 'tray resume while hidden')
    menu('Open ClipSync')
    wait(lambda: int(ui('count windows')) > 0, 'tray reopen after red close')
    menu('Quit ClipSync app')
    assert app.wait(timeout=8) == 0
    assert request(session,'Status')['pid'] == session['pid'], 'client quit stopped the daemon'
    result={'passed':['native close hides to menu bar','tray pause and resume while hidden','tray reopens native window','Quit exits client and preserves daemon']}
    out.joinpath('results.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
finally:
    if app.poll() is None:
        app.send_signal(signal.SIGINT)
        try:app.wait(timeout=5)
        except subprocess.TimeoutExpired:app.kill();app.wait()
    if request(session,'Status')['status']['paused']:
        request(session,{'SetPause':{'paused':False}})
