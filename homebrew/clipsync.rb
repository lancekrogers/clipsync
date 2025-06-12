class Clipsync < Formula
  desc "Secure clipboard synchronization across devices"
  homepage "https://github.com/lancekrogers/clipsync"
  license "MIT"
  head "https://github.com/lancekrogers/clipsync.git", branch: "main"

  # Source tarball
  url "https://github.com/lancekrogers/clipsync/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "PLACEHOLDER_SHA256_SOURCE"

  # Build dependencies
  depends_on "rust" => :build
  depends_on "pkg-config" => :build
  
  # Runtime dependencies
  depends_on "openssl@3"
  
  # Platform-specific dependencies
  on_macos do
    depends_on xcode: ["14.0", :build]
  end
  
  on_linux do
    depends_on "libx11"
    depends_on "libxcb"
  end

  def install
    # Set OpenSSL paths for macOS
    if OS.mac?
      ENV["OPENSSL_ROOT_DIR"] = Formula["openssl@3"].opt_prefix
      ENV["OPENSSL_LIB_DIR"] = Formula["openssl@3"].opt_lib
      ENV["OPENSSL_INCLUDE_DIR"] = Formula["openssl@3"].opt_include
    end

    # Build the project
    system "cargo", "install", *std_cargo_args
  end

  def post_install
    # Create necessary directories
    (var/"log").mkpath
    (var/"lib/clipsync").mkpath
    (etc/"clipsync").mkpath
  end

  service do
    run [opt_bin/"clipsync", "start", "--daemon"]
    keep_alive true
    log_path var/"log/clipsync.log"
    error_log_path var/"log/clipsync.error.log"
    environment_variables PATH: std_service_path_env
  end

  test do
    assert_match "clipsync", shell_output("#{bin}/clipsync --version")
    system bin/"clipsync", "status"
  end

  def caveats
    <<~EOS
      ClipSync has been installed!

      Quick start:
        clipsync --help        # Show help
        clipsync status        # Check status
        brew services start clipsync  # Start service

      Building from source? Use:
        brew install --build-from-source clipsync
    EOS
  end
end