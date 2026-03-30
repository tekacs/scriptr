class Scriptr < Formula
  desc "Fast, caching launcher for Rust single-file packages"
  homepage "https://github.com/tekacs/scriptr"
  version "0.1.4"
  license "MIT OR Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/tekacs/scriptr/releases/download/v0.1.4/scriptr-0.1.4-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER"
    end
    on_intel do
      url "https://github.com/tekacs/scriptr/releases/download/v0.1.4/scriptr-0.1.4-x86_64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/tekacs/scriptr/releases/download/v0.1.4/scriptr-0.1.4-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER"
    end
    on_intel do
      url "https://github.com/tekacs/scriptr/releases/download/v0.1.4/scriptr-0.1.4-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER"
    end
  end

  def install
    bin.install "scriptr"
  end

  test do
    system "#{bin}/scriptr", "--version"
  end
end
