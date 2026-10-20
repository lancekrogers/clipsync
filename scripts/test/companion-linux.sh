#!/usr/bin/env bash
# Runs only in the disposable container created by just companion-test-linux.
set -euo pipefail
mode=${1:?x11 or wayland}
out=${2:?evidence directory}
mkdir -p "$out"
export XDG_RUNTIME_DIR
XDG_RUNTIME_DIR=$(mktemp -d /tmp/clipsync-runtime.XXXXXX)
chmod 700 "$XDG_RUNTIME_DIR"
export CLIPSYNC_DISPOSABLE_DESKTOP=1 LIBGL_ALWAYS_SOFTWARE=1
export EGUI_INSPECTION=127.0.0.1:18762 CLIPSYNC_UI_PORT=18762
export CLIPSYNC_BINARY=/work/target/debug/clipsync
export CLIPSYNC_UI_SESSION="$out/session.json"
cleanup() {
  for pid in ${app:-} ${fixture:-} ${desktop:-}; do kill "$pid" 2>/dev/null || true; done
  rm -rf "$XDG_RUNTIME_DIR"
}
trap cleanup EXIT
if [[ $mode == x11 ]]; then
  export DISPLAY=:94
  unset WAYLAND_DISPLAY
  Xvfb "$DISPLAY" -screen 0 1200x900x24 > "$out/desktop.log" 2>&1 & desktop=$!
elif [[ $mode == wayland ]]; then
  unset DISPLAY
  export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1
  printf 'output * resolution 1200x900\nseat seat0 fallback true\nfor_window [title="^ClipSync$"] floating enable\n' > "$XDG_RUNTIME_DIR/sway.conf"
  sway -c "$XDG_RUNTIME_DIR/sway.conf" > "$out/desktop.log" 2>&1 & desktop=$!
  for i in {1..50}; do
    [[ ! -S $XDG_RUNTIME_DIR/wayland-1 ]] || break
    sleep .1
  done
  export WAYLAND_DISPLAY=wayland-1
else
  echo 'Expected x11 or wayland' >&2; exit 2
fi
sleep .3
rm -f "$out/fixture.jsonl"
/work/target/debug/examples/companion_fixture > "$out/fixture.jsonl" 2> "$out/fixture.log" & fixture=$!
for i in {1..300}; do
  [[ ! -s $out/fixture.jsonl ]] || break
  if ! kill -0 "$fixture" 2>/dev/null; then
    cat "$out/fixture.log" >&2
    echo "Isolated fixture exited before publishing its socket" >&2
    exit 1
  fi
  sleep .1
done
if [[ ! -s $out/fixture.jsonl ]]; then
  cat "$out/fixture.log" >&2
  echo "Isolated fixture startup timed out" >&2
  exit 1
fi
python3 - "$out" <<'PY'
import json,sys
from pathlib import Path
out=Path(sys.argv[1]); session=json.loads(out.joinpath('fixture.jsonl').read_text().splitlines()[0]);session['pasteboard']='disposable-linux'
out.joinpath('session.json').write_text(json.dumps(session))
PY
config=$(python3 -c 'import json,os;print(json.load(open(os.environ["CLIPSYNC_UI_SESSION"]))["config"])')
if [[ $(id -u) == 0 ]]; then
  python3 scripts/test/companion_control_uid.py "$out/peer-credentials"
fi
/work/target/debug/clipsync-desktop --config "$config" --no-tray > "$out/app.log" 2>&1 & app=$!
python3 scripts/test/companion_ui.py "$out/gui"
python3 scripts/test/companion_pty.py "$CLIPSYNC_BINARY" "$out/tui"
python3 scripts/test/companion_recovery.py "$CLIPSYNC_BINARY" "$out/recovery"

python3 scripts/test/companion_large_history.py "$out/large-history"
