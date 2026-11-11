# Sourced by demos/companion-tui.tape inside VHS. Starts the isolated companion
# fixture (private pasteboard, temporary identity, loopback peer, seeded
# history) and points `clipsync` at it, so recordings never show this
# machine's paths, clipboard or devices.

# Refuse until the fixture is up, so a failed start never reaches an
# installed clipsync and the real daemon.
clipsync() { echo "Demo fixture is not running." >&2; return 1; }
if [ "$(uname -s)" != Darwin ]; then
  echo "The demo fixture needs a private macOS pasteboard; record on macOS." >&2
  return 1
fi
demo_bin="$(cd "${CARGO_TARGET_DIR:-target}/debug" && pwd)" || return 1
demo_dir="$(mktemp -d "${TMPDIR:-/tmp}/clipsync-demo.XXXXXX")" || return 1
export CLIPSYNC_TEST_PASTEBOARD="org.clipsync.test.$(uuidgen)"
export CLIPSYNC_PEER_FIXTURE=1 CLIPSYNC_IMAGE_FIXTURE=1 CLIPSYNC_IMAGE_DETAIL_FIXTURE=1
# VHS reports Sixel support but records only text cells.
export CLIPSYNC_TUI_GRAPHICS=off
export TERM=xterm-256color COLORTERM=truecolor
unset NO_COLOR TERM_PROGRAM TMUX
"$demo_bin/examples/companion_fixture" >"$demo_dir/session.json" 2>"$demo_dir/fixture.log" &
demo_fixture=$!
echo "$demo_fixture" >target/demo/fixture.pid
for _ in $(seq 100); do
  [ -s "$demo_dir/session.json" ] && break
  sleep 0.1
done
demo_config="$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["config"])' "$demo_dir/session.json")"
if [ -z "$demo_config" ] || [ ! -f "$demo_config" ]; then
  echo "Demo fixture did not start; see $demo_dir/fixture.log" >&2
  return 1
fi
clipsync() { "$demo_bin/clipsync" --config "$demo_config" "$@"; }
demo_stop() { kill -INT "$demo_fixture" && wait "$demo_fixture"; }
PS1='$ '
