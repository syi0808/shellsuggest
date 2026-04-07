class Shellsuggest < Formula
  desc "Smarter zsh autosuggestions — ranked by your current directory"
  homepage "https://github.com/syi0808/shellsuggest"
  version "0.1.0"
  license "Apache-2.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/syi0808/shellsuggest/releases/download/v0.1.0/shellsuggest-darwin-arm64.tar.gz"
      sha256 "PLACEHOLDER"
    elsif Hardware::CPU.intel?
      url "https://github.com/syi0808/shellsuggest/releases/download/v0.1.0/shellsuggest-darwin-x64.tar.gz"
      sha256 "PLACEHOLDER"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/syi0808/shellsuggest/releases/download/v0.1.0/shellsuggest-linux-arm64.tar.gz"
      sha256 "PLACEHOLDER"
    elsif Hardware::CPU.intel?
      url "https://github.com/syi0808/shellsuggest/releases/download/v0.1.0/shellsuggest-linux-x64.tar.gz"
      sha256 "PLACEHOLDER"
    end
  end

  def install
    bin.install "shellsuggest"
  end

  test do
    system "#{bin}/shellsuggest", "--version"
  end
end
