"""Drive the actual eframe window through test-only inspection (synthetic data)."""
import json, os, socket, struct, sys, time
from pathlib import Path
import msgpack
out=Path(sys.argv[1]);out.mkdir(parents=True,exist_ok=True)
port=int(os.environ.get('CLIPSYNC_UI_PORT','18762'))
for attempt in range(80):
    try:
        s=socket.create_connection(('127.0.0.1',port),timeout=3);break
    except OSError:time.sleep(.15)
else:raise RuntimeError('No inspection listener')
s.settimeout(10)
def read(n):
    result=b''
    while len(result)<n:
        chunk=s.recv(n-len(result))
        if not chunk:raise EOFError('Inspection disconnected')
        result+=chunk
    return result
assert read(8)==b'eins\0\0\0\1'
def req(value):
    b=msgpack.packb(value,use_bin_type=True);s.sendall(struct.pack('>I',len(b))+b)
    size=struct.unpack('>I',read(4))[0];assert size<64*1024*1024
    result=msgpack.unpackb(read(size),raw=False,strict_map_key=False)
    assert not isinstance(result,dict) or 'Error' not in result,result
    return result
def tree():return req('GetTree')['Tree']
def nodes():return tree()[2][0]
def text():return json.dumps(tree(),default=lambda x:list(x))
def find(label):
    for ident,node in nodes():
        for value in node:
            if isinstance(value,dict) and str(value.get('label','')).strip()==label.strip() and 'bounds' in value:return value['bounds']
    raise AssertionError(f'No control {label}: {text()}')
mods=dict(alt=False,ctrl=False,shift=False,mac_cmd=False,command=False)
def events(values):return req({'ApplyEvents':[values]})
viewport_height=800
def click(label):
    min_y=390 if '\n' in label else 110
    for _ in range(12):
        x,y,r,b=find(label)
        if y>=min_y and b<=viewport_height-10:break
        delta=-260 if b>viewport_height-10 else 260
        events([{'PointerMoved':[(x+r)/2,min(500,viewport_height-80)]},{'MouseWheel':dict(unit='Point',delta=[0,delta],phase='Move',source='Wheel',modifiers=mods)}]);time.sleep(.2)
    x,y,r,b=find(label);pos=[(x+r)/2,(y+b)/2]
    assert 0<=pos[1]<viewport_height, f'{label} is outside viewport: {pos}'
    events([{'PointerMoved':pos},{'PointerButton':dict(pos=pos,button='Primary',pressed=True,modifiers=mods)}])
    time.sleep(.1)
    events([{'PointerButton':dict(pos=pos,button='Primary',pressed=False,modifiers=mods)}])
    time.sleep(.1)
def key(name):
    for pressed in (True,False):events([{'Key':dict(key=name,physical_key=None,pressed=pressed,repeat=False,modifiers=mods)}])
def shot(name):
    value=req({'GetScreenshot':[1.0]})['Screenshot'];out.joinpath(name+'.png').write_bytes(value[1])

def wait_for(predicate, message):
    deadline=time.monotonic()+8
    while time.monotonic()<deadline:
        if predicate():return
        time.sleep(.1)
    raise AssertionError(message+": "+text())
def resize(width,height):
    global viewport_height
    req({'Resize':[width,height]})
    viewport_height=height
    def size_matches():
        png=req({'GetScreenshot':[1.0]})['Screenshot'][1]
        return struct.unpack('>II',png[16:24])==(width,height)
    wait_for(size_matches, f'actual viewport must become {width}x{height}')
def find_copy(fragment):
    for ident,node in nodes():
        if node[0]!='button':continue
        for value in node:
            if isinstance(value,dict) and fragment in value.get('label',''):
                return value['label']
    raise AssertionError('No saved copy containing '+fragment)
def replace_search(query):
    click('Search clipboard history')
    command=dict(mods,command=True,mac_cmd=sys.platform=='darwin',ctrl=sys.platform!='darwin')
    for pressed in (True,False):events([{'Key':dict(key='A',physical_key=None,pressed=pressed,repeat=False,modifiers=command)}])
    events([{'Text':query}])

session=json.loads(Path(os.environ['CLIPSYNC_UI_SESSION']).read_text())
wait_for(lambda:'Ready to sync' in text(), 'daemon ready')
shot('overview')
click('Pause syncing')
wait_for(lambda:'Resume syncing' in text(), 'pause applies')
click('Resume syncing')
wait_for(lambda:'Pause syncing' in text(), 'resume applies')
click('History')
wait_for(lambda:'cargo test --locked' in text(), 'real history loads')
click(find_copy('cargo test --locked'))
wait_for(lambda:'Copy text' in text(), 'selected detail loads')
time.sleep(.3)
shot('history-selected')
click('Copy text')
wait_for(lambda:'Queued for' in text() or 'Copied locally' in text(), 'copy acknowledgement')
import subprocess
copy_env=os.environ|{'CLIPSYNC_TEST_PASTEBOARD':session['pasteboard']}
if sys.platform=='darwin':
    output=subprocess.check_output(['swift',str(Path(__file__).with_name('read_private_pasteboard.swift')),session['pasteboard']],text=True)
else:
    output=subprocess.check_output([os.environ['CLIPSYNC_BINARY'],'--config',session['config'],'paste'],env=copy_env,text=True)
assert 'cargo test --locked' in output,output
if os.environ.get('CLIPSYNC_IMAGE_FIXTURE') == '1' and sys.platform == 'darwin':
    click(find_copy('[Image]'))
    wait_for(lambda:'Copy image' in text(), 'image detail loads')
    wait_for(lambda:'Copied image preview' in text(), 'image preview renders')
    assert not any(isinstance(v,dict) and str(v.get('label','')).strip()=='[Image]' for _,node in nodes() for v in node), 'preview replaces the [Image] placeholder'
    shot('history-image-selected')
    click('Copy image')
    if os.environ.get('CLIPSYNC_IMAGE_FAILURE_FIXTURE') == '1':
        wait_for(lambda:'The clipboard may have changed. Check it before copying again.' in text(), 'uncertain image write warning')
        assert 'Copied locally' not in text(), 'uncertain write must not report success'
        shot('history-image-write-unknown')
    elif os.environ.get('CLIPSYNC_IMAGE_HISTORY_FAILURE_FIXTURE') == '1':
        wait_for(lambda:'The clipboard changed, but this copy could not be saved to history.' in text(), 'image history failure warning')
        assert 'Copied locally' not in text(), 'partial failure must not report full success'
        import sqlite3
        with sqlite3.connect(str(Path(session['config']).parent / 'history.db')) as db:
            assert db.execute("SELECT count(*) FROM clipboard_history WHERE content_type = 'image/png'").fetchone()[0] == 1
        shot('history-image-history-failed')
    else:
        wait_for(lambda:'Queued for' in text() or 'Copied locally' in text(), 'image copy acknowledgement')
    # The text copy's notice can still be showing, so wait for the image itself.
    deadline=time.monotonic()+8
    while True:
        image=subprocess.check_output(['swift',str(Path(__file__).with_name('read_private_pasteboard.swift')),session['pasteboard'],'--png-base64'],text=True).strip()
        if image or time.monotonic()>deadline:break
        time.sleep(.2)
    if os.environ.get('CLIPSYNC_IMAGE_DETAIL_FIXTURE') == '1':
        import base64
        png=base64.b64decode(image)
        assert png[:8]==b'\x89PNG\r\n\x1a\n' and struct.unpack('>II',png[16:24])==(640,400), png[:24]
    else:
        assert image == 'iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAAEUlEQVR4nGP4z8Dwn6HhfwMAEPoD/o3Nc3sAAAAASUVORK5CYII=', image
replace_search('review')
wait_for(lambda:'Review notes' in text() and 'cargo test --locked' not in text(), 'filter results')
shot('history-search')
replace_search('nothing-matches-this')
wait_for(lambda:'No matching copies' in text(), 'empty search recovery')
assert 'Copy text' not in text(), 'hidden selection should not remain copyable'
shot('history-empty')
click('Clear search')
wait_for(lambda:'cargo test --locked' in text(), 'clear search')
click(find_copy('cargo test --locked'));time.sleep(.3)
resize(800,640)
find('Copy text');shot('history-narrow')
before=tree()[2][-1];key('Tab');after=tree()[2][-1];assert before!=after,'Tab did not move focus'
resize(1080,800)
click('Settings');click('Show diagnostics')
wait_for(lambda:'clipsync-diagnostics-v1' in text(),'diagnostics')
assert 'cargo test --locked' not in text(),'clipboard data in diagnostics'
click('History');shot('history-final')
results={'passed':['native daemon status','pause and resume through actual IPC','real encrypted history list and detail','Copy verified independently on isolated native clipboard','search and empty-state recovery','hidden selection cleared','800x640 layout retains Copy action','Tab focus movement','redacted diagnostics'], 'info':req('GetInfo')}
if os.environ.get('CLIPSYNC_IMAGE_FIXTURE') == '1' and sys.platform == 'darwin':
    if os.environ.get('CLIPSYNC_IMAGE_FAILURE_FIXTURE') == '1':
        results['passed'].append('uncertain image write warning and native pasteboard verified')
    elif os.environ.get('CLIPSYNC_IMAGE_HISTORY_FAILURE_FIXTURE') == '1':
        results['passed'].append('image history failure warning, unchanged history, and native pasteboard verified')
    else:
        results['passed'].append('image history copy verified on isolated native pasteboard')
out.joinpath('results.json').write_text(json.dumps(results,indent=2)+'\n')
print(json.dumps(results,indent=2))
