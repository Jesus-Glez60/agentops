#!/usr/bin/env node
"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const http = require("http");
const https = require("https");
const { execFileSync } = require("child_process");
const spawn = require("cross-spawn");
const tar = require("tar");

const platform = require("../lib/platform");
const pkg = require("../package.json");

function fail(message) {
  console.error(`agentops-cli: ${message}`);
  process.exit(1);
}

// GitHub release asset URLs redirect (to a signed S3 URL) -- https.get()
// does not follow redirects itself, so this walks the chain manually
// rather than pulling in a request library just for this.
function download(url, destPath, redirectsLeft = 5) {
  return new Promise((resolve, reject) => {
    const client = url.startsWith("http://") ? http : https;
    client
      .get(url, { headers: { "User-Agent": "agentops-cli-npm-shim" } }, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          res.resume();
          if (redirectsLeft <= 0) return reject(new Error("too many redirects"));
          return resolve(download(res.headers.location, destPath, redirectsLeft - 1));
        }
        if (res.statusCode !== 200) {
          res.resume();
          return reject(new Error(`download failed: HTTP ${res.statusCode} for ${url}`));
        }
        const file = fs.createWriteStream(destPath);
        res.pipe(file);
        file.on("finish", () => file.close(resolve));
        file.on("error", reject);
      })
      .on("error", reject);
  });
}

async function extract(archivePath, target, destDir) {
  fs.mkdirSync(destDir, { recursive: true });
  if (platform.isWindows(target)) {
    // Windows 10 1803+ ships bsdtar as `tar.exe`, which extracts .zip
    // natively -- avoids adding a second (zip-specific) npm dependency.
    execFileSync("tar", ["-xf", archivePath, "-C", destDir], { stdio: "inherit" });
  } else {
    await tar.x({ file: archivePath, cwd: destDir });
  }
}

async function ensureBinaryInstalled(target) {
  const binPath = platform.binaryPath(target);
  const stampPath = platform.versionStampPath();
  const currentStamp = fs.existsSync(stampPath) ? fs.readFileSync(stampPath, "utf8").trim() : null;

  if (fs.existsSync(binPath) && currentStamp === pkg.version) {
    return binPath;
  }

  console.error(`agentops-cli: fetching agentops ${pkg.version} for ${target}...`);
  const tmpFile = path.join(os.tmpdir(), `agentops-cli-${process.pid}-${platform.assetName(target)}`);
  try {
    await download(platform.downloadUrl(pkg.version, target), tmpFile);
    await extract(tmpFile, target, platform.binDir());
  } finally {
    fs.rmSync(tmpFile, { force: true });
  }

  if (!platform.isWindows(target)) {
    fs.chmodSync(binPath, 0o755);
  }
  fs.writeFileSync(platform.versionStampPath(), pkg.version);
  return binPath;
}

async function main() {
  const target = platform.resolveTarget();
  if (!target) {
    fail(`unsupported platform ${os.platform()}/${os.arch()}. See https://github.com/Jesus-Glez60/agentops#readme for manual install options.`);
  }

  let binPath;
  try {
    binPath = await ensureBinaryInstalled(target);
  } catch (err) {
    fail(`couldn't install the agentops binary: ${err.message}`);
    return;
  }

  const result = spawn.sync(binPath, process.argv.slice(2), { stdio: "inherit" });
  if (result.error) {
    fail(`failed to run agentops: ${result.error.message}`);
  }
  process.exit(result.status ?? 1);
}

main();
