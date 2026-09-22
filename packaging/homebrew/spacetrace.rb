class Spacetrace < Formula
  desc "Scan a disk, snapshot it, and find out what grew"
  homepage "https://spacetrace.teknobakkall.com"
  version "@VERSION@"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/unalcakir28/spacetrace/releases/download/v@VERSION@/spacetrace-v@VERSION@-aarch64-apple-darwin.tar.gz"
      sha256 "@SHA_DARWIN_ARM64@"
    end
    on_intel do
      url "https://github.com/unalcakir28/spacetrace/releases/download/v@VERSION@/spacetrace-v@VERSION@-x86_64-apple-darwin.tar.gz"
      sha256 "@SHA_DARWIN_X86_64@"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/unalcakir28/spacetrace/releases/download/v@VERSION@/spacetrace-v@VERSION@-aarch64-unknown-linux-musl.tar.gz"
      sha256 "@SHA_LINUX_ARM64@"
    end
    on_intel do
      url "https://github.com/unalcakir28/spacetrace/releases/download/v@VERSION@/spacetrace-v@VERSION@-x86_64-unknown-linux-musl.tar.gz"
      sha256 "@SHA_LINUX_X86_64@"
    end
  end

  def install
    # Both binaries ship in every archive, at its root. The agent is the half
    # that runs on a server; someone installing the CLI on their laptop will
    # not start it, and leaving it out would mean a second way to install.
    bin.install "spacetrace"
    bin.install "spacetrace-agent"
  end

  test do
    # The version, not just a zero exit status: a formula that fetched the
    # wrong release still runs, and this is the assertion that notices.
    assert_match version.to_s, shell_output("#{bin}/spacetrace --version")
    assert_match version.to_s, shell_output("#{bin}/spacetrace-agent --version")

    # And one real scan, because a binary that starts is not a binary that
    # works — the macOS builds in particular can fail at first syscall if the
    # archive was built for the other architecture. The file goes at the root:
    # the listing ranks the root's own children, so one inside a subdirectory
    # would be reported as the subdirectory and this would assert nothing.
    (testpath/"probe.txt").write("x" * 2048)
    assert_match "probe.txt", shell_output("#{bin}/spacetrace scan #{testpath} --top 10")
  end
end
