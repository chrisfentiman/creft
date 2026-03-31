#!/usr/bin/env node

// Downloads the creft binary for the current platform from GitHub Releases.
// Uses only Node.js built-ins — no dependencies needed.

const { execSync } = require("child_process");
const fs = require("fs");
const https = require("https");
const path = require("path");

const VERSION = require("./package.json").version;
const REPO = "chrisfentiman/creft";

const PLATFORM_MAP = {
  "darwin-arm64": "aarch64-apple-darwin",
  "darwin-x64": "x86_64-apple-darwin",
  "linux-x64": "x86_64-unknown-linux-gnu",
  "linux-arm64": "aarch64-unknown-linux-gnu",
};

function getTarget() {
  const key = `${process.platform}-${process.arch}`;
  const target = PLATFORM_MAP[key];
  if (!target) {
    console.error(`Unsupported platform: ${key}`);
    console.error(`Supported: ${Object.keys(PLATFORM_MAP).join(", ")}`);
    process.exit(1);
  }
  return target;
}

function fetch(url) {
  return new Promise((resolve, reject) => {
    https
      .get(url, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          return fetch(res.headers.location).then(resolve, reject);
        }
        if (res.statusCode !== 200) {
          return reject(new Error(`HTTP ${res.statusCode} for ${url}`));
        }
        resolve(res);
      })
      .on("error", reject);
  });
}

async function install() {
  const target = getTarget();
  const filename = `creft-${VERSION}-${target}.tar.gz`;
  const url = `https://github.com/${REPO}/releases/download/creft-v${VERSION}/${filename}`;

  const binDir = path.join(__dirname, "bin");
  fs.mkdirSync(binDir, { recursive: true });

  const tarball = path.join(binDir, filename);

  console.log(`Downloading creft v${VERSION} for ${target}...`);

  const res = await fetch(url);

  // Write tarball to disk
  await new Promise((resolve, reject) => {
    const file = fs.createWriteStream(tarball);
    res.pipe(file);
    file.on("finish", () => file.close(resolve));
    file.on("error", reject);
  });

  // Extract using system tar (available on macOS and Linux)
  execSync(`tar xzf "${tarball}" -C "${binDir}"`, { stdio: "inherit" });
  fs.unlinkSync(tarball);

  const binary = path.join(binDir, "creft");
  fs.chmodSync(binary, 0o755);
  console.log(`Installed creft to ${binary}`);
}

install().catch((err) => {
  console.error(`Failed to install creft: ${err.message}`);
  process.exit(1);
});
