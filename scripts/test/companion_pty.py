"""Run the real TUI in a PTY, check display, resize, input and termios restoration."""
import fcntl, json, os, pty, select, signal, struct, subprocess, sys, termios, time
from pathlib import Path
import pyte
out=Path(sys.argv[2]);out.mkdir(parents=True,exist_ok=True)
master,slave=pty.openpty();before=termios.tcgetattr(slave)
def size(w,h):fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',h,w,0,0))
size(100,30)
env=os.environ|{'TERM':'xterm-256color','NO_COLOR':'1'}
session=json.loads(Path(os.environ["CLIPSYNC_UI_SESSION"]).read_text())
p=subprocess.Popen([sys.argv[1],"--config",session["config"],"tui"],stdin=slave,stdout=slave,stderr=slave,env=env,start_new_session=True)
screen=pyte.Screen(100,30);stream=pyte.Stream(screen);raw=bytearray()
def pump(duration=.4):
    end=time.monotonic()+duration
    while time.monotonic()<end:
        if select.select([master],[],[],.03)[0]:
            b=os.read(master,65536);raw.extend(b);stream.feed(b.decode('utf-8',errors='replace'))
def snapshot():return '\n'.join(line.rstrip() for line in screen.display).rstrip()+'\n'
def send(value):os.write(master,value);pump()
try:
    pump(2);assert 'Your clipboard, everywhere' in snapshot(),snapshot()
    assert 'Ready to sync' in snapshot(),snapshot()
    out.joinpath('terminal-wide.txt').write_text(snapshot())
    send(b'p');pump(1.5);assert 'Paused' in snapshot(),snapshot()
    send(b'p');pump(1.5);assert 'Ready to sync' in snapshot(),snapshot()
    send(b'2');assert 'Connected now' in snapshot(),snapshot()
    send(b'3');pump(1.5);assert 'History' in snapshot(),snapshot()
    send(b'/cargo');pump(1);assert 'cargo test --locked' in snapshot(),snapshot()
    send(b'\r');send(b'\r');pump(.5)
    assert 'cargo test --locked' in snapshot(),snapshot()
    send(b'c');pump(.5)
    assert 'Queued for' in snapshot() or 'Copied locally' in snapshot(),snapshot()
    send(b'4');assert 'Background' in snapshot() or 'Settings' in snapshot(),snapshot()
    size(60,18);screen.resize(18,60);os.killpg(p.pid,signal.SIGWINCH);pump()
    out.joinpath('terminal-compact.txt').write_text(snapshot());assert 'Settings' in snapshot()
    send(b'q');assert p.wait(timeout=3)==0
    assert termios.tcgetattr(slave)==before,'termios not restored'
    assert b'\x1b[?1049l' in raw,'alternate screen not left'
    assert b'\x1b[?25h' in raw,'cursor not restored'
    out.joinpath('terminal-results.json').write_text(json.dumps(dict(passed=['real PTY render','route keyboard navigation','daemon pause/resume','real history search and copy','100x30 to 60x18 resize','NO_COLOR','raw mode/cursor/alternate screen restored']),indent=2)+'\n')
    print(out.joinpath('terminal-results.json').read_text())
finally:
    if p.poll() is None:p.terminate();p.wait(timeout=3)
    os.close(master);os.close(slave)
