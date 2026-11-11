# Sourced by demos/companion-tui-pairing.tape inside VHS. Starts two isolated
# companion fixtures (private pasteboards, temporary identities, loopback only)
# and defines `pair_demo`, which shows both terminal companions side by side
# in a private tmux server: Studio Mac allows a device, Travel laptop joins.
# Ctrl-T in tmux types the code Studio Mac is showing into the right pane, the
# way a person would read it off the other screen.

pair_demo() { echo "Demo fixtures are not running." >&2; return 1; }
if [ "$(uname -s)" != Darwin ]; then
  echo "The demo fixtures need private macOS pasteboards; record on macOS." >&2
  return 1
fi
demo_repo="$(pwd)"
demo_bin="$(cd "${CARGO_TARGET_DIR:-target}/debug" && pwd)" || return 1
demo_dir="$(mktemp -d "${TMPDIR:-/tmp}/clipsync-pairing-demo.XXXXXX")" || return 1
export CLIPSYNC_TUI_GRAPHICS=off TERM=xterm-256color COLORTERM=truecolor
unset NO_COLOR TERM_PROGRAM TMUX CLIPSYNC_PEER_FIXTURE CLIPSYNC_UI_SESSION EGUI_INSPECTION
demo_port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"

CLIPSYNC_TEST_PASTEBOARD="org.clipsync.test.$(uuidgen)" CLIPSYNC_FIXTURE_NAME="Studio Mac" \
  CLIPSYNC_FIXTURE_ENROLL_PORT="$demo_port" \
  "$demo_bin/examples/companion_fixture" >"$demo_dir/studio.json" 2>"$demo_dir/studio.log" &
demo_studio=$!
CLIPSYNC_TEST_PASTEBOARD="org.clipsync.test.$(uuidgen)" CLIPSYNC_FIXTURE_NAME="Travel laptop" \
  CLIPSYNC_FIXTURE_NEARBY="Studio Mac@127.0.0.1:$demo_port" \
  "$demo_bin/examples/companion_fixture" >"$demo_dir/laptop.json" 2>"$demo_dir/laptop.log" &
demo_laptop=$!
echo "$demo_studio $demo_laptop" >target/demo/fixture.pid
for _ in $(seq 100); do
  [ -s "$demo_dir/studio.json" ] && [ -s "$demo_dir/laptop.json" ] && break
  sleep 0.1
done
demo_config() { python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["config"])' "$1"; }
studio_config="$(demo_config "$demo_dir/studio.json")"
laptop_config="$(demo_config "$demo_dir/laptop.json")"
if [ ! -f "$studio_config" ] || [ ! -f "$laptop_config" ]; then
  echo "Demo fixtures did not start; see $demo_dir/*.log" >&2
  return 1
fi

# Reads the live code from Studio Mac's own control socket and types it,
# digit by digit, into Travel laptop's pane.
cat >"$demo_dir/type-code.sh" <<EOF
#!/bin/sh
code=\$(python3 -c 'import json, sys
sys.path.insert(0, sys.argv[1])
from control_client import request
session = json.load(open(sys.argv[2]))
print(request(session, {"EnrollmentInspect": {"session_id": None}})["enrollment"]["code"]["code"])' "$demo_repo/scripts/test" "$demo_dir/studio.json") || exit 1
for digit in \$(echo "\$code" | fold -w1); do
  tmux -L clipsync-demo send-keys -t pair:0.1 "\$digit"
  sleep 0.4
done
EOF
chmod +x "$demo_dir/type-code.sh"

cat >"$demo_dir/tmux.conf" <<EOF
set -g status off
set -g default-terminal "xterm-256color"
set -ga terminal-overrides ",*:Tc"
set -sg escape-time 0
set -g allow-rename off
set -g set-titles off
set -g mouse off
set -g pane-border-status top
set -g pane-border-style "fg=#3a3f4b"
set -g pane-active-border-style "fg=#69bfff"
set -g pane-border-format "#{?#{==:#{pane_index},0}, Studio Mac · allows a device ,  Travel laptop · joins }"
bind -n C-t run-shell -b "$demo_dir/type-code.sh"
EOF

pair_demo() {
  tmux -L clipsync-demo -f "$demo_dir/tmux.conf" new-session -s pair \
    "'$demo_bin/clipsync' --config '$studio_config' tui" \; \
    split-window -h "'$demo_bin/clipsync' --config '$laptop_config' tui" \; \
    select-pane -L
}
demo_stop() {
  tmux -L clipsync-demo kill-server 2>/dev/null
  kill -INT "$demo_studio" "$demo_laptop" && wait "$demo_studio" "$demo_laptop"
}
PS1='$ '
