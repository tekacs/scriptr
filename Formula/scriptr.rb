class Scriptr < Formula
  desc "Fast, caching launcher for Rust single-file packages"
  homepage "https://github.com/tekacs/scriptr"
  version "0.1.5"
  license "MIT OR Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/tekacs/scriptr/releases/download/v0.1.5/scriptr-0.1.5-aarch64-apple-darwin.tar.gz"
      sha256 "030a7cf396dcd70150079e8e71d38a9519d9e1202d6205c373843ee29b59960d"
    end
    on_intel do
      url "https://github.com/tekacs/scriptr/releases/download/v0.1.5/scriptr-0.1.5-x86_64-apple-darwin.tar.gz"
      sha256 "3d3ad467645c6a5b0d7027278fcdcb0376a96c599a995e6c8e70893d1575ca7f"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/tekacs/scriptr/releases/download/v0.1.5/scriptr-0.1.5-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "9b820af63446ccecacc01353697c4254e9815ff36a265542da5416865d86b467"
    end
    on_intel do
      url "https://github.com/tekacs/scriptr/releases/download/v0.1.5/scriptr-0.1.5-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "45f16186031dc970c28a31cf47c9a49f3da48e4e5165dc61a53870f46a389b89"
    end
  end

  def install
    bin.install "scriptr"
  end

  test do
    system "#{bin}/scriptr", "--version"
  end
end
