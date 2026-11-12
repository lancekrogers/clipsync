# What syncs: current scope and limits

- Plain text up to 5 MiB; automatic polling every 250 ms.
- Image sync for PNG clipboard data on macOS, X11, and Wayland data-control; macOS TIFF clipboard images are converted to PNG. Image files copied as files are not synced.
- macOS NSPasteboard, Linux X11, and Wayland compositors supporting data-control through arboard. Arboard can fall back to X11 where available.
- Explicit peer authorization, automatic discovery, bounded reconnect backoff, stale/duplicate event rejection, and remote-update echo suppression.
- The latest local event is retained in memory for newly connected sessions. Starting a daemon does not automatically broadcast pre-existing clipboard contents; use `sync` to request that.
- AES-256-GCM encrypted history **contents**. Database metadata is not encrypted. The current retention limit is 20 entries.
- Separate `status`, `peers`, and `sync` commands communicate with the daemon through a protected per-user Unix socket. `sync` reports **queued** peers, not confirmed remote application.

RTF synchronization, global hotkeys, and primary-selection configuration are not supported by the repaired sync engine. Sensitive-content filtering is a heuristic and does not inspect text inside images; it is not a guarantee that every secret will be detected. Trusted peers receive copied images, and image history contents are encrypted like text history.

Image sync is experimental. On Linux, a stalled image write may finish after its
timeout and replace a newer clipboard copy. Native application paste and color
handling still need broader testing. Images are limited by the configured byte
limit and the receiving peer, with an additional maximum of 8,192 pixels per
dimension and 16,777,216 pixels in total. The current queue budget also caps an
encoded image to approximately 12.8 MiB even if `clipboard.max_size` is higher.

The transport reserves frame capacity for each connection. With the default
5 MiB clipboard limit, the default ten connections fit; raising
`clipboard.max_size` to 50 MiB permits one connection at a time, including for
text-only peers. The reservation controls admission, not total process memory.
