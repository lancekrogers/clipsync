"""Real first-run GUI; setup writes only under a disposable child-process HOME.
Never clicks install/start/login controls or accesses a clipboard.
"""
import json
import os
import signal
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path
from inspection_client import Inspection

root = Path(__file__).resolve().parents[2]
out = Path(sys.argv[1]);out.mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(prefix='clipsync-first-run-') as directory:
    home = Path(directory)
    env = os.environ | {'HOME':str(home),'XDG_CONFIG_HOME':str(home/'.config'),
        'XDG_DATA_HOME':str(home/'.local/share'),'XDG_STATE_HOME':str(home/'.local/state'),
        'EGUI_INSPECTION':'127.0.0.1:18765'}
    env.pop('CLIPSYNC_CONFIG',None)
    app = subprocess.Popen([str(root/'target/debug/clipsync-desktop'),'--no-tray'],env=env,
        stdout=out.joinpath('app.log').open('w'),stderr=subprocess.STDOUT)
    try:
        ui=Inspection(18765)
        ui.wait(lambda:'Set up this computer' in ui.text(),'first-run setup appears')
        ui.screenshot(out/'welcome.png')
        ui.click('Set up this computer')
        ui.wait(lambda:'Open service settings' in ui.text(),'fresh setup reaches unavailable-service state')
        config=home/'.config/clipsync/config.toml'
        parsed=tomllib.loads(config.read_text())
        identity=Path(parsed['auth']['ssh_key']);auth=Path(parsed['auth']['authorized_keys'])
        key=Path(parsed['clipboard']['history_key'])
        assert key.is_relative_to(home)
        assert not key.exists(), "History encryption key is created only by the daemon"
        for path in (identity,auth,config):
            assert path.is_relative_to(home),path
            assert path.is_file(),path
            assert path.stat().st_mode & 0o077 == 0,path
        assert identity.with_suffix('.pub').is_file()
        original=identity.read_bytes()
        ui.click('Open service settings')
        ui.wait(lambda:'Install service' in ui.text(),'next step is explicit service installation')
        ui.screenshot(out/'setup-ready.png')
        ui.click('Check again')
        ui.wait(lambda:'Your existing identity' in ui.text(),'existing setup adoption')
        assert identity.read_bytes()==original
        assert not (home/'Library/LaunchAgents/com.clipsync.daemon.plist').exists()
        result={'passed':['native first-run setup','private persisted identity and authorization; explicit future history key path','fresh config paths stay inside fixture','reinspection preserves identity','service installation requires separate explicit action']}
        out.joinpath('results.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
    finally:
        if app.poll() is None:
            app.send_signal(signal.SIGINT)
            try:app.wait(timeout=5)
            except subprocess.TimeoutExpired:app.kill();app.wait()
