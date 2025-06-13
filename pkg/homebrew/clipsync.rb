class Clipsync < Formula
  desc "Cross-platform clipboard synchronization service"
  homepage "https://github.com/lancekrogers/clipsync"
  version "0.1.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    if Hardware::CPU.intel?
      url "https://github.com/lancekrogers/clipsync/releases/download/v#{version}/clipsync-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256_INTEL"
    else
      url "https://github.com/lancekrogers/clipsync/releases/download/v#{version}/clipsync-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256_ARM"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "https://github.com/lancekrogers/clipsync/releases/download/v#{version}/clipsync-#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_SHA256_LINUX_INTEL"
    else
      url "https://github.com/lancekrogers/clipsync/releases/download/v#{version}/clipsync-#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_SHA256_LINUX_ARM"
    end
  end

  def install
    bin.install "clipsync"
    
    # Install launchd plist for macOS
    if OS.mac?
      (prefix/"com.clipsync.plist").write plist_content
    end
  end

  def post_install
    # Create config directory
    (var/"clipsync").mkpath
    (etc/"clipsync").mkpath
  end

  service do
    if OS.mac?
      name macos: "com.clipsync"
      keep_alive true
      log_path var/"log/clipsync.log"
      error_log_path var/"log/clipsync.error.log"
      working_dir var/"clipsync"
    else
      run [opt_bin/"clipsync"]
      keep_alive true
      log_path var/"log/clipsync.log"
      error_log_path var/"log/clipsync.error.log"
      working_dir var/"clipsync"
    end
  end

  def plist_content
    <<~EOS
      <?xml version="1.0" encoding="UTF-8"?>
      <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
      <plist version="1.0">
      <dict>
        <key>Label</key>
        <string>com.clipsync</string>
        <key>ProgramArguments</key>
        <array>
          <string>#{opt_bin}/clipsync</string>
        </array>
        <key>RunAtLoad</key>
        <true/>
        <key>KeepAlive</key>
        <true/>
        <key>StandardOutPath</key>
        <string>#{var}/log/clipsync.log</string>
        <key>StandardErrorPath</key>
        <string>#{var}/log/clipsync.error.log</string>
        <key>WorkingDirectory</key>
        <string>#{var}/clipsync</string>
      </dict>
      </plist>
    EOS
  end

  test do
    assert_match "ClipSync", shell_output("#{bin}/clipsync --version")
  end

  def caveats
    <<~EOS
      To start ClipSync as a background service:
        brew services start clipsync

      To stop ClipSync:
        brew services stop clipsync

      Configuration files are stored in:
        #{etc}/clipsync

      Log files are stored in:
        #{var}/log/clipsync.log
    EOS
  end
end