# Task 14: Packaging and Distribution

## Objective
Set up packaging for Homebrew (macOS) and AUR (Arch Linux), plus binary releases.

## Steps

1. **Create Homebrew formula**
   ```ruby
   class Clipsync < Formula
     desc "Cross-platform clipboard synchronization with history"
     homepage "https://github.com/yourusername/clipsync"
     url "https://github.com/yourusername/clipsync/archive/v0.1.0.tar.gz"
     sha256 "..."
     license "MIT"
     
     depends_on "rust" => :build
     
     def install
       system "cargo", "install", *std_cargo_args
     end
     
     service do
       run [opt_bin/"clipsync", "daemon"]
       keep_alive true
       log_path var/"log/clipsync.log"
       error_log_path var/"log/clipsync-error.log"
     end
   end
   ```

2. **Create AUR PKGBUILD**
   ```bash
   pkgname=clipsync
   pkgver=0.1.0
   pkgrel=1
   pkgdesc="Cross-platform clipboard synchronization with history"
   arch=('x86_64' 'aarch64')
   url="https://github.com/yourusername/clipsync"
   license=('MIT')
   depends=('libx11' 'wayland')
   makedepends=('rust' 'cargo')
   source=("$pkgname-$pkgver.tar.gz::$url/archive/v$pkgver.tar.gz")
   
   build() {
     cd "$pkgname-$pkgver"
     cargo build --release --locked
   }
   
   package() {
     cd "$pkgname-$pkgver"
     install -Dm755 "target/release/clipsync" "$pkgdir/usr/bin/clipsync"
     install -Dm644 "scripts/clipsync.service" "$pkgdir/usr/lib/systemd/user/clipsync.service"
   }
   ```

3. **GitHub Release workflow**
   - Build for all platforms
   - Create signed binaries
   - Generate checksums
   - Upload artifacts

4. **Create install script**
   - Detect platform
   - Download appropriate binary
   - Set up service files
   - Configure permissions

5. **Documentation**
   - Installation guide
   - Configuration examples
   - Troubleshooting guide
   - API documentation

6. **Release checklist**
   - Version bump
   - Changelog update
   - Tag creation
   - Package updates

## Success Criteria
- Clean installation on both platforms
- Services start automatically
- Upgrade path works
- Documentation is complete