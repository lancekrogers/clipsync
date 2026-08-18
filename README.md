# clipsync

**Copy on this machine. Paste on the other. No cloud.**

LAN clipboard sync for macOS and Linux. Peers find each other with
mDNS, authenticate with SSH keys, and encrypt the payload
(AES-256-GCM). Text, RTF, and images up to 5 MB. Early software;
expect breakage.

```
laptop copy  →  LAN  →  desktop paste
```

## Install

```bash
# from source (honest path today)
git clone https://github.com/lancekrogers/clipsync.git
cd clipsync && cargo install --path .

# optional tap
brew tap lancekrogers/clipsync
brew install lancekrogers/clipsync/clipsync
```

Needs Rust 1.75+. Linux also wants `libx11-dev libxcb-dev libssl-dev`.

## Two machines

```bash
ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519_clipsync   # once per box
clipsync start
clipsync status
```

When the other host shows up:

```
New device discovered: laptop (192.168.1.100:8484)
SSH Fingerprint: SHA256:…
Do you want to trust this device? (T)rust/(R)eject/(I)gnore:
```

Type `T`. Copy here, paste there. History: `Ctrl+Shift+V` (⌘ on macOS).

More setup: [docs/INSTALL.md](docs/INSTALL.md).

## Commands

```bash
clipsync start [--foreground]
clipsync stop
clipsync status
clipsync copy "hello"
clipsync paste
clipsync sync
clipsync history --interactive
clipsync peers
clipsync doctor
clipsync logs
```

Config is `~/.config/clipsync/config.toml` (`clipsync config init`).

| Hotkey | |
|--------|--|
| `Ctrl+Shift+V` | History picker |
| `Ctrl+Shift+C` | Secondary clipboard |
| `Ctrl+Shift+S` | Force sync |
| `Ctrl+Shift+[` / `]` | Prev / next |

## Why not Syncthing / a pastebin

Those leave the LAN or write a file. This is the clipboard itself,
peer to peer, with a trust prompt that looks like SSH.

## Docs

| | |
|--|--|
| [Install](docs/INSTALL.md) | Packages and from source |
| [User guide](docs/USER_GUIDE.md) | Day to day |
| [Config](docs/CONFIG.md) | Every key |
| [Troubleshooting](docs/TROUBLESHOOTING.md) | `doctor`, no peers, no paste |
| [Security](docs/SECURITY.md) | Keys, AES-GCM, no telemetry |

## License

MIT or Apache-2.0. See [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE).
