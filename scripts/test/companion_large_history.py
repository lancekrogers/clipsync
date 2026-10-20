"""Real UI pagination keeps long Unicode copies readable without changing Copy."""
import json
import os
import subprocess
import sys
from pathlib import Path
from control_client import request
from inspection_client import Inspection

out=Path(sys.argv[1]);out.mkdir(parents=True,exist_ok=True)
session=json.loads(Path(os.environ['CLIPSYNC_UI_SESSION']).read_text())
ui=Inspection(18762)
large='Large fixture copy\n' + ('🧪 Unicode clipboard text.\n'*900) + 'END OF ORIGINAL COPY'
request(session,{'Copy':{'text':large}})
ui.click('History')
ui.wait(lambda:'Large fixture copy' in ui.text(),'new long copy appears')
label=next(value['label'] for _,node in ui.tree()[2][0] for value in node if isinstance(value,dict) and 'Large fixture copy' in str(value.get('label','')))
ui.click(label)
ui.wait(lambda:'Next page' in ui.text(),'long copy pages')
assert 'END OF ORIGINAL COPY' not in ui.text()
for _ in range(8):
    if 'END OF ORIGINAL COPY' in ui.text():break
    ui.click('Next page')
else:raise AssertionError('last page unreachable')
ui.click('Copy text')
ui.wait(lambda:'Copied locally' in ui.text() or 'Queued for' in ui.text(),'long copy acknowledged')
if sys.platform=='darwin':
    copied=subprocess.check_output(['swift',str(Path(__file__).with_name('read_private_pasteboard.swift')),session['pasteboard']],text=True)
else:
    copied=subprocess.check_output([os.environ['CLIPSYNC_BINARY'],'--config',session['config'],'paste'],text=True)
assert large in copied,'Copy must use original complete text, not the visible page'
ui.screenshot(out/'last-page.png')
result={'passed':['long Unicode history displays in bounded pages','last page reachable','Copy preserves original complete text across pages']}
out.joinpath('results.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
