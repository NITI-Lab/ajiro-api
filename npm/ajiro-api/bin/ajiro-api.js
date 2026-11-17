#!/usr/bin/env node
"use strict";

// Thin launcher: finds the platform package that holds the native binary
// (installed through optionalDependencies) and runs it with inherited stdio.

const { spawnSync } = require("node:child_process");
const path = require("node:path");

const PLATFORMS = {
  "linux-x64": "@niti-lab/ajiro-api-linux-x64",
  "linux-arm64": "@niti-lab/ajiro-api-linux-arm64",
  "darwin-x64": "@niti-lab/ajiro-api-darwin-x64",
  "darwin-arm64": "@niti-lab/ajiro-api-darwin-arm64",
  "win32-x64": "@niti-lab/ajiro-api-win32-x64",
};

function resolveBinary() {
  // Escape hatch for development and unusual installs.
  if (process.env.AJIRO_API_BINARY) return process.env.AJIRO_API_BINARY;

  const key = `${process.platform}-${process.arch}`;
  const pkg = PLATFORMS[key];
  if (!pkg) {
    fail(`Unsupported platform: ${key}. Supported: ${Object.keys(PLATFORMS).join(", ")}.`);
  }
  const exe = process.platform === "win32" ? "ajiro-api.exe" : "ajiro-api";
  try {
    const pkgJson = require.resolve(`${pkg}/package.json`);
    return path.join(path.dirname(pkgJson), "bin", exe);
  } catch (_) {
    fail(
      `The native binary package ${pkg} is not installed.\n` +
        "This usually means optional dependencies were skipped (e.g. --no-optional / --omit=optional)\n" +
        "or the install ran on a different OS or CPU architecture.\n" +
        "Fixes: reinstall with optional dependencies enabled, or install another way:\n" +
        "  cargo install ajiro-api\n" +
        "  https://github.com/niti-lab/ajiro-api/releases"
    );
  }
}

function fail(message) {
  process.stderr.write(`ajiro-api: ${message}\n`);
  process.exit(2);
}

function main() {
  const binary = resolveBinary();
  const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
  if (result.error) {
    fail(`failed to run ${binary}: ${result.error.message}`);
  }
  if (result.signal) {
    // Mirror the signal so shells and CI see the same termination reason.
    process.kill(process.pid, result.signal);
    return;
  }
  process.exit(result.status === null ? 1 : result.status);
}

main();
