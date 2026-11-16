# Formula template. The release workflow replaces the __PLACEHOLDERS__ and
# pushes the result to the Homebrew tap (niti-lab/homebrew-tap, Formula/ajiro-api.rb).
class AjiroApi < Formula
  desc "API definitions, regression tests and mocks for AI coding agents"
  homepage "https://github.com/niti-lab/ajiro-api"
  version "__VERSION__"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/niti-lab/ajiro-api/releases/download/v__VERSION__/ajiro-api-__VERSION__-aarch64-apple-darwin.tar.gz"
      sha256 "__SHA256_AARCH64_DARWIN__"
    else
      url "https://github.com/niti-lab/ajiro-api/releases/download/v__VERSION__/ajiro-api-__VERSION__-x86_64-apple-darwin.tar.gz"
      sha256 "__SHA256_X86_64_DARWIN__"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/niti-lab/ajiro-api/releases/download/v__VERSION__/ajiro-api-__VERSION__-aarch64-unknown-linux-musl.tar.gz"
      sha256 "__SHA256_AARCH64_LINUX__"
    else
      url "https://github.com/niti-lab/ajiro-api/releases/download/v__VERSION__/ajiro-api-__VERSION__-x86_64-unknown-linux-musl.tar.gz"
      sha256 "__SHA256_X86_64_LINUX__"
    end
  end

  def install
    bin.install "ajiro-api"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/ajiro-api --version")
  end
end
