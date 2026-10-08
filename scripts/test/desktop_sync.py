"""Actual daemon acceptance on disposable X11 desktops; never run on a personal desktop."""
import os
import pathlib
import subprocess
import tempfile
import time
import sys

if not pathlib.Path('/.dockerenv').exists():
    raise SystemExit('This harness requires the disposable test container')
wayland = '--wayland' in sys.argv
binary = str(pathlib.Path('target/debug/clipsync').resolve())
children = []
logs = []

def run(args, **kwargs):
    return subprocess.run(args, check=True, timeout=15, **kwargs)

def wait_for(predicate, description, seconds=30):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        if predicate():
            return
        time.sleep(0.2)
    raise AssertionError(description)

with tempfile.TemporaryDirectory(prefix='clipsync-desktops-') as temporary:
    root = pathlib.Path(temporary)
    try:
        configs, environments, daemon_processes = [], [], []
        for i in range(2):
            directory = root / str(i)
            directory.mkdir()
            if wayland:
                runtime = directory/'runtime'
                runtime.mkdir(mode=0o700)
                sway_config = directory/'sway.conf'
                sway_config.write_text('output * resolution 800x600\nseat seat0 fallback true\n')
                env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime), WLR_BACKENDS='headless', WLR_RENDERER='pixman', WLR_LIBINPUT_NO_DEVICES='1')
                env.pop('DISPLAY', None)
                env.pop('WAYLAND_DISPLAY', None)
                log = open(root/f'sway-{i}.log', 'w+')
                logs.append(log)
                children.append(subprocess.Popen(['sway', '-c', str(sway_config)], env=env, stdout=log, stderr=log))
                wait_for(lambda: any(p.is_socket() for p in runtime.glob('wayland-*')), 'headless compositor failed')
                env['WAYLAND_DISPLAY'] = next(p.name for p in runtime.glob('wayland-*') if p.is_socket())
            else:
                display = f':{91+i}'
                env = dict(os.environ, DISPLAY=display)
                env.pop('WAYLAND_DISPLAY', None)
                children.append(subprocess.Popen(['Xvfb', display, '-screen', '0', '800x600x24'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
            run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-f', str(directory/'identity')])
            configs.append(directory/'config.toml')
            environments.append(env)
            configs[-1].write_text(f'''listen_addr = "0.0.0.0:{18484+i}"
[auth]
ssh_key = "{directory/'identity'}"
authorized_keys = "{directory/'authorized_keys'}"
[clipboard]
history_db = "{directory/'history.db'}"
history_key = "{directory/'history.key'}"
''')
        for i in range(2):
            (root/str(i)/'authorized_keys').write_text((root/str(1-i)/'identity.pub').read_text())
        time.sleep(0.5)
        for env in environments:
            run(['wl-copy', '--type', 'text/plain'] if wayland else ['xclip', '-selection', 'clipboard'], env=env, input=b'initial fixture text')
        def cli(i, *args):
            return subprocess.run([binary, '--config', str(configs[i]), *args], env=environments[i], capture_output=True, text=True, timeout=8)
        def start(i):
            log = open(root/f'daemon-{i}-{len(logs)}.log', 'w+')
            logs.append(log)
            process = subprocess.Popen([binary, '--config', str(configs[i]), 'start', '--foreground'], env=environments[i], stdout=log, stderr=log)
            children.append(process)
            return process
        daemon_processes = [start(0), start(1)]
        wait_for(lambda: all(cli(i, 'status').returncode == 0 for i in range(2)), 'daemons did not become ready')
        wait_for(lambda: all('Connected peers: 1' in cli(i, 'peers').stdout for i in range(2)), 'automatic discovery/TLS connection failed')
        def paste(i):
            result = subprocess.run(['wl-paste', '--no-newline', '--type', 'text/plain'] if wayland else ['xclip', '-selection', 'clipboard', '-o'], env=environments[i], capture_output=True, timeout=3)
            return result.stdout.decode()
        run(['wl-copy', '--type', 'text/plain'] if wayland else ['xclip', '-selection', 'clipboard'], env=environments[0], input=b'alpha actual desktop')
        wait_for(lambda: paste(1) == 'alpha actual desktop', 'A to B automatic sync failed')
        run(['wl-copy', '--type', 'text/plain'] if wayland else ['xclip', '-selection', 'clipboard'], env=environments[1], input=b'beta actual desktop')
        wait_for(lambda: paste(0) == 'beta actual desktop', 'B to A automatic sync failed')
        result = cli(0, 'sync')
        assert result.returncode == 0 and 'queued for 1' in result.stdout, result.stdout + result.stderr
        assert 'actual desktop' in cli(1, 'history').stdout
        original_key = (root/'1'/'history.key').read_bytes()
        daemon_processes[1].terminate()
        daemon_processes[1].wait(timeout=10)
        assert cli(1, 'status').returncode != 0
        daemon_processes[1] = start(1)
        wait_for(lambda: all('Connected peers: 1' in cli(i, 'peers').stdout for i in range(2)), 'restart did not reconnect')
        assert (root/'1'/'history.key').read_bytes() == original_key
        assert 'actual desktop' in cli(1, 'history').stdout
        run(['wl-copy', '--type', 'text/plain'] if wayland else ['xclip', '-selection', 'clipboard'], env=environments[0], input=b'after daemon restart')
        wait_for(lambda: paste(1) == 'after daemon restart', 'sync after restart failed')
        print(f'PASS: two actual {"Wayland" if wayland else "X11"} daemons, OpenSSH identities, non-default ports, automatic discovery, bidirectional clipboard, separate CLI, history reopen, restart/reconnect', flush=True)
    except BaseException:
        for log in logs:
            log.flush()
            log.seek(0)
            print(log.read()[-16000:], flush=True)
        raise
    finally:
        for process in reversed(children):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        for log in logs:
            log.close()
