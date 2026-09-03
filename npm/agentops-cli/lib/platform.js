"use strict";

const os = require("os");
const path = require("path");

// Must match .github/workflows/release.yml's build matrix and install.sh's
// `${platform_arch}-${platform_os}` convention exactly, since both this
// shim and install.sh download from the same GitHub Release assets and
// must resolve to the same cached binary path.
// No "darwin:x64" (Intel Mac) -- ort-sys 2.0.0-rc.13 ships no prebuilt
// ONNX Runtime binary for x86_64-apple-darwin on any host, so
// release.yml's build matrix doesn't produce that asset at all. An Intel
// Mac falls through to `resolveTarget`'s `null` case below and gets a
// clear "unsupported platform" error instead of a 404 mid-download.
const TARGETS = {
  "darwin:arm64": "aarch64-apple-darwin",
  "linux:x64": "x86_64-unknown-linux-gnu",
  "linux:arm64": "aarch64-unknown-linux-gnu",
  "win32:x64": "x86_64-pc-windows-msvc",
};

function resolveTarget(platform = os.platform(), arch = os.arch()) {
  return TARGETS[`${platform}:${arch}`] || null;
}

function isWindows(target) {
  return target.endsWith("-pc-windows-msvc");
}

// `install.sh`'s own default: `${AGENTOPS_INSTALL_DIR:-$HOME/.agentops}` --
// kept identical so a machine already set up via `curl | sh` is detected
// and reused here with zero re-download, and vice versa.
function installDir() {
  return process.env.AGENTOPS_INSTALL_DIR || path.join(os.homedir(), ".agentops");
}

function binDir() {
  return path.join(installDir(), "bin");
}

function binaryPath(target) {
  return path.join(binDir(), isWindows(target) ? "agentops.exe" : "agentops");
}

function versionStampPath() {
  return path.join(binDir(), ".agentops-cli-version");
}

function assetName(target) {
  return isWindows(target) ? `agentops-${target}.zip` : `agentops-${target}.tar.gz`;
}

// Pinned to the exact release tag matching this npm package's own version --
// never `/latest/download/...`, which would let `npx agentops-cli@0.3.0`
// silently fetch a newer binary once a later release ships.
function downloadUrl(version, target) {
  const base = process.env.AGENTOPS_CLI_DOWNLOAD_BASE_URL || "https://github.com/Jesus-Glez60/agentops/releases/download";
  return `${base.replace(/\/+$/, "")}/v${version}/${assetName(target)}`;
}

module.exports = { TARGETS, resolveTarget, isWindows, installDir, binDir, binaryPath, versionStampPath, assetName, downloadUrl };
