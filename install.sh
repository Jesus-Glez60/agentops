#!/usr/bin/env bash
# Classic terminal deployment installer (Method 3): downloads the latest
# release's agentops/agentops-server/agentops-mcp-server binaries and the
# Next.js standalone frontend tarball (see .github/workflows/release.yml),
# extracts them, then hands off to `agentops init` for the interactive
# setup wizard.
#
#   curl -fsSL https://raw.githubusercontent.com/Jesus-Glez60/agentops/main/install.sh | sh
#
# Not cargo-dist-generated (see release.yml's header comment on why) --
# a plain, auditable shell script covering the same job: detect
# platform, download the matching release asset, extract to
# ~/.agentops/bin and ~/.agentops/web.
set -euo pipefail

REPO="Jesus-Glez60/agentops"
INSTALL_DIR="${AGENTOPS_INSTALL_DIR:-$HOME/.agentops}"

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Linux) platform_os="unknown-linux-gnu" ;;
  Darwin) platform_os="apple-darwin" ;;
  *)
    echo "error: unsupported OS: $os (this installer covers Linux and macOS; see .github/workflows/release.yml for the Windows .zip asset)" >&2
    exit 1
    ;;
esac

case "$arch" in
  x86_64 | amd64) platform_arch="x86_64" ;;
  arm64 | aarch64) platform_arch="aarch64" ;;
  *)
    echo "error: unsupported architecture: $arch" >&2
    exit 1
    ;;
esac

target="${platform_arch}-${platform_os}"
asset="agentops-${target}.tar.gz"
release_url="https://github.com/${REPO}/releases/latest/download"

echo "Installing agentops (${target}) into ${INSTALL_DIR}..."
mkdir -p "$INSTALL_DIR/bin" "$INSTALL_DIR/web"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL "${release_url}/${asset}" -o "$tmp/agentops.tar.gz"
tar -xzf "$tmp/agentops.tar.gz" -C "$INSTALL_DIR/bin"

if curl -fsSL "${release_url}/agentops-web-standalone.tar.gz" -o "$tmp/web.tar.gz" 2>/dev/null; then
  tar -xzf "$tmp/web.tar.gz" -C "$INSTALL_DIR/web"
  echo "Installed the web UI to ${INSTALL_DIR}/web."
  if ! command -v node >/dev/null 2>&1; then
    echo "warning: node was not found on PATH -- the web UI (${INSTALL_DIR}/web/server.js) needs a Node.js runtime to run, even though agentops-server itself doesn't. Install Node, or run 'agentops init' for a backend-only setup." >&2
  fi
else
  echo "warning: no bundled web UI release asset found -- installed the CLI/backend only." >&2
fi

echo ""
echo "Installed: ${INSTALL_DIR}/bin/{agentops,agentops-server,agentops-mcp-server}"
echo "Add it to your PATH, then run:"
echo "  export PATH=\"${INSTALL_DIR}/bin:\$PATH\""
echo "  agentops init"
