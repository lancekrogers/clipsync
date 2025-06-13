# ClipSync – Cross-Platform Clipboard Sync Service (Rust)

## 1 Purpose & Scope

- Provide **near-real-time** clipboard synchronisation between macOS and Linux (X11 & Wayland) machines on the same network.
- Support text, rich-text/RTF, and image payloads ≤ 5 MB (configurable).
- Maintain a 20-item clipboard history with secure disk persistence.
- Deliver a single static binary per platform with minimal external dependencies.
- Support both X11 and Wayland on Linux with automatic detection.

---

## 2 Goals & Success Metrics

| Goal                                | Metric                                                                   |
| ----------------------------------- | ------------------------------------------------------------------------ |
| Seamless copy/paste both directions | < 500 ms median latency for ≤ 100 KB text; < 2s for 5 MB payloads        |
| Secure transport                    | SSH key-based authentication with perfect forward secrecy                 |
| Lightweight                         | < 30 MB RSS, < 2 % CPU at idle (including history cache)                 |
| Easy install                        | `brew install` / `pacman -S` packages; zero-config LAN discovery         |

---

## 3 User Stories

- **As a developer** I want to copy code on my Mac and paste on my Arch laptop.
- **As a power user** I want to disable sync temporarily with a hot-key.
- **As a privacy-conscious user** I want traffic encrypted and clipboard history encrypted on disk.
- **As a power user** I want to cycle through my last 20 clipboard entries with hotkeys.

---

## 4 High-Level Architecture

```
+-----------------+        mDNS/DNS-SRV        +-----------------+
|  ClipSync Node  |<-------------------------->|  ClipSync Node  |
| (macOS server)  |---SSH-WebSocket (ws+ssh)-->| (Linux client)  |
+-----------------+     Binary Protocol        +-----------------+
        |                                               |
        v                                               v
+------------------+                          +------------------+
| Clipboard History |                          | Clipboard History |
| (20 items, AES)   |                          | (20 items, AES)   |
+------------------+                          +------------------+
```

Every node runs the same binary; the first started with `--serve` acts as the authority for conflict resolution.

### 4.1 Core Modules

| Module            | Responsibility                                                    |
| ----------------- | ----------------------------------------------------------------- |
| `clipboard`       | Abstract clipboard interface; macOS & x11/wayland implementations |
| `watcher`         | Detect local clipboard changes (Wayland signals or 150 ms poll)   |
| `transport`       | WebSocket with SSH-based auth, streaming chunks, reconnection     |
| `proto`           | Binary protocol with streaming support, 1s chunk deadline         |
| `history`         | SQLite DB for 20 items, AES-256-GCM encryption, keyring integration |
| `registry`        | mDNS service advertisement & peer discovery                       |
| `config`          | Load TOML from `~/.config/clipsync/config.toml`                  |
| `cli`             | CLI entrypoint & flags                                            |
| `tray` (optional) | System-tray UI via `egui` or `tauri`                             |

---

## 5 Clipboard Sync Protocol

```rust
// Binary protocol using bincode/msgpack
struct ClipboardMessage {
    msg_type: MessageType,
    id: Uuid,
    mimetype: String,    // text/plain, image/png, text/rtf
    total_size: u64,     // total payload size
    chunk_index: u16,    // for streaming large payloads
    chunk_count: u16,    // total number of chunks
    checksum: [u8; 32],  // SHA-256 of complete payload
    payload: Vec<u8>,    // chunk data (max 64KB per chunk)
    timestamp: i64,
    origin: Uuid,
    history_index: u8,   // position in history ring (0 = most recent)
}

enum MessageType {
    ClipboardUpdate,
    HistoryRequest,
    HistoryResponse,
    Heartbeat,
}
```

- Nodes ignore frames where `origin == self` → prevents loops.
- Payloads exceeding `max_size` (default 5 MB) are rejected with error.
- Chunks must complete within 1 second or transfer is aborted.
- History cycling via `Ctrl+Shift+V` (configurable) shows selection UI.

---

## 6 Configuration Schema (TOML)

```toml
listen_addr = ":8484"
advertise_name = "user-macbook"

[auth]
ssh_key = "~/.ssh/id_ed25519"  # For peer authentication only
authorized_keys = "~/.config/clipsync/authorized_keys"

[clipboard]
max_size = 5_242_880  # bytes (5 MB)
sync_primary = true   # middle-click selection (Linux)
history_size = 20
history_db = "~/.local/share/clipsync/history.db"  # SQLite database

[hotkeys]
toggle_sync = "Ctrl+Shift+Cmd+C"     # macOS
# toggle_sync = "Ctrl+Shift+Alt+C"   # Linux
show_history = "Ctrl+Shift+V"
cycle_prev = "Ctrl+Shift+["
cycle_next = "Ctrl+Shift+]"

[security]
# AES-256-GCM key storage (in order of preference):
# 1. System keyring (automatic)
# 2. SSH-encrypted file at ~/.config/clipsync/history.key
# 3. Prompt for passphrase (argon2id derivation)
encryption = "aes-256-gcm"
compression = "zstd"  # for large payloads

log_level = "info"
```

---

## 7 Build & Packaging

| Platform   | Target                               | Notes                                      |
| ---------- | ------------------------------------ | ------------------------------------------ |
| macOS      | `aarch64-apple-darwin`, `x86_64-apple-darwin` | launchd plist, notarized DMG        |
| Arch Linux | `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` | systemd unit; AUR package |

Use **Rust 1.75+** with `cargo` and `just` for builds. Key dependencies:
- `arboard` or `copypasta-ext` for clipboard access
- `tokio` for async runtime
- `quinn` or `tokio-tungstenite` for transport
- `ssh2` or `russh` for SSH authentication
- `rusqlite` with `bundled-sqlcipher` for encrypted history
- `keyring` for secure AES key storage
- `zeroize` for secure memory handling
- `argon2` for key derivation (fallback)

---

## 8 Testing Strategy

- **Unit tests:** Mock clipboard traits, test history ring buffer, encryption.
- **Integration tests:** Docker containers with X11/Wayland, test both display servers.
- **CI:** GitHub Actions matrix (`macos-latest`, `ubuntu-latest` with X11 and Wayland).
- **Security:** `cargo-audit`, `cargo-deny`, fuzzing with `cargo-fuzz`.
- **Clipboard compatibility:** Test with common apps (VS Code, terminals, browsers).

---

## 9 Roadmap

| Milestone | Features                                                           |
| --------- | ------------------------------------------------------------------ |
| **v0.1**  | Text sync, SSH auth, X11 support, basic CLI                       |
| **v0.2**  | Wayland support, 20-item history, encrypted persistence            |
| **v0.3**  | Streaming for 5MB payloads, image/RTF support, mDNS discovery     |
| **v0.4**  | History UI with hotkeys, system tray, auto-reconnect              |
| **v1.0**  | Signed binaries, Homebrew/AUR packages, compression, benchmarks   |

---

## 10 Database Schema

```sql
-- SQLite schema for clipboard history
CREATE TABLE clipboard_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    uuid TEXT NOT NULL UNIQUE,
    content BLOB NOT NULL,          -- AES-256-GCM encrypted
    content_type TEXT NOT NULL,     -- text/plain, text/rtf, image/png, etc
    content_size INTEGER NOT NULL,  -- Original size before encryption
    checksum TEXT NOT NULL,         -- SHA-256 of plaintext
    timestamp INTEGER NOT NULL,     -- Unix timestamp
    origin_node TEXT NOT NULL,      -- Node UUID that created this entry
    iv BLOB NOT NULL,               -- AES initialization vector
    created_at INTEGER DEFAULT (strftime('%s', 'now'))
);

CREATE INDEX idx_timestamp ON clipboard_history(timestamp DESC);
CREATE INDEX idx_content_type ON clipboard_history(content_type);

-- Trigger to maintain 20 item limit
CREATE TRIGGER limit_history_size
AFTER INSERT ON clipboard_history
BEGIN
    DELETE FROM clipboard_history
    WHERE id IN (
        SELECT id FROM clipboard_history
        ORDER BY timestamp DESC
        LIMIT -1 OFFSET 20
    );
END;
```

### Encryption Key Management

1. **Primary**: System keyring (macOS Keychain, Linux Secret Service)
   ```rust
   keyring::Entry::new("clipsync", "history_key")
   ```

2. **Fallback**: SSH-encrypted key file
   ```rust
   // Generate on first run
   let aes_key = rand::random::<[u8; 32]>();
   let encrypted = ssh_pubkey.encrypt(&aes_key);
   fs::write("~/.config/clipsync/history.key", encrypted);
   ```

3. **Last resort**: Password derivation
   ```rust
   argon2::hash_password(passphrase, salt, 32)
   ```

---

## 11 Stretch Goals

- Windows support via WinAPI clipboard.
- Drag-and-drop file transfer up to 10 MB.
- P2P sync without central server (distributed consensus).
- Clipboard history search with FTS (full-text search).
- Mobile companion app for iOS/Android.
- Browser extension for web clipboard sync.
