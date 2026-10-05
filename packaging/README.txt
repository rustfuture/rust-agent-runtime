rust-agent-runtime (rust-agent-runtime)
=======================================

Runs automated coding tasks with execution timeouts, command restrictions and verification tests. This archive holds the `rust-agent-runtime` command-line tool, built from the tagged release.

Install
-------
Put the `rust-agent-runtime` binary (`rust-agent-runtime.exe` on Windows) in a directory on your PATH,
or use the installer, which does that for you and checks the checksum:

  macOS / Linux:  curl -fsSL https://raw.githubusercontent.com/rustfuture/rust-agent-runtime/main/install.sh | sh
  Windows:        irm https://raw.githubusercontent.com/rustfuture/rust-agent-runtime/main/install.ps1 | iex

Then:

  rust-agent-runtime --version
  rust-agent-runtime --help

Verify this download
--------------------
Each archive has a matching .sha256 file on the release page:

  sha256sum -c rust-agent-runtime-<version>-<target>.tar.gz.sha256      (Linux)
  shasum -a 256 -c rust-agent-runtime-<version>-<target>.tar.gz.sha256  (macOS)

Docs and source: https://github.com/rustfuture/rust-agent-runtime
License: see LICENSE
