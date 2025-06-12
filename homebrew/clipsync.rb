class Clipsync < Formula
  desc "Secure clipboard synchronization across devices"
  homepage "https://github.com/lancekrogers/clipsync"
  version "0.1.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/lancekrogers/clipsync/releases/download/v0.1.0/clipsync-macos-aarch64.tar.gz"
      sha256 "PLACEHOLDER_SHA256_AARCH64"
    else
      url "https://github.com/lancekrogers/clipsync/releases/download/v0.1.0/clipsync-macos-x86_64.tar.gz"
      sha256 "PLACEHOLDER_SHA256_X86_64"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/lancekrogers/clipsync/releases/download/v0.1.0/clipsync-linux-aarch64.tar.gz"
      sha256 "PLACEHOLDER_SHA256_LINUX_AARCH64"
    else
      url "https://github.com/lancekrogers/clipsync/releases/download/v0.1.0/clipsync-linux-x86_64.tar.gz"
      sha256 "PLACEHOLDER_SHA256_LINUX_X86_64"
    end
  end

  def install
    bin.install "clipsync"
  end

  service do
    run [opt_bin/"clipsync", "start", "--daemon"]
    keep_alive true
    log_path var/"log/clipsync.log"
    error_log_path var/"log/clipsync.error.log"
  end

  test do
    assert_match "ClipSync", shell_output("#{bin}/clipsync --version")
  end
end