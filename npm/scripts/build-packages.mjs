#!/usr/bin/env node
// Generate the npm packages from release archives.
//
//   node npm/scripts/build-packages.mjs --version 0.1.0 --dist dist --out npm/out
//
// Input : <dist>/ajiro-api-<version>-<rust-target>.tar.gz|zip (from release.yml)
// Output: <out>/cli-<platform>-<arch>/   (@niti-lab/ajiro-api-<platform>-<arch>)
//         <out>/ajiro-api/               (main package, versions pinned)

import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync, chmodSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { mkdtempSync } from "node:fs";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..", "..");

// Rust target -> npm platform/arch (process.platform / process.arch values)
export const TARGETS = [
  { target: "x86_64-unknown-linux-musl", platform: "linux", arch: "x64", ext: "tar.gz", os: ["linux"], cpu: ["x64"] },
  { target: "aarch64-unknown-linux-musl", platform: "linux", arch: "arm64", ext: "tar.gz", os: ["linux"], cpu: ["arm64"] },
  { target: "x86_64-apple-darwin", platform: "darwin", arch: "x64", ext: "tar.gz", os: ["darwin"], cpu: ["x64"] },
  { target: "aarch64-apple-darwin", platform: "darwin", arch: "arm64", ext: "tar.gz", os: ["darwin"], cpu: ["arm64"] },
  { target: "x86_64-pc-windows-msvc", platform: "win32", arch: "x64", ext: "zip", os: ["win32"], cpu: ["x64"] },
];

function parseArgs(argv) {
  const args = { version: null, dist: "dist", out: "npm/out", allowMissing: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--version") args.version = argv[++i];
    else if (a === "--dist") args.dist = argv[++i];
    else if (a === "--out") args.out = argv[++i];
    else if (a === "--allow-missing") args.allowMissing = true;
    else throw new Error(`unknown argument: ${a}`);
  }
  if (!args.version) throw new Error("--version is required");
  return args;
}

function extract(archive, destDir, ext) {
  mkdirSync(destDir, { recursive: true });
  if (ext === "zip") {
    try {
      execFileSync("unzip", ["-q", "-o", archive, "-d", destDir], { stdio: "inherit" });
    } catch {
      // Windows runners and some images have bsdtar, which reads zip.
      execFileSync("tar", ["-xf", archive, "-C", destDir], { stdio: "inherit" });
    }
  } else {
    execFileSync("tar", ["-xzf", archive, "-C", destDir], { stdio: "inherit" });
  }
}

function findFile(dir, name) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, entry.name);
    if (entry.isDirectory()) {
      const found = findFile(p, name);
      if (found) return found;
    } else if (entry.name === name) {
      return p;
    }
  }
  return null;
}

function main() {
  const { version, dist, out, allowMissing } = parseArgs(process.argv.slice(2));
  const distDir = resolve(dist);
  const outDir = resolve(out);
  rmSync(outDir, { recursive: true, force: true });
  mkdirSync(outDir, { recursive: true });

  const mainPkg = JSON.parse(readFileSync(join(root, "npm", "ajiro-api", "package.json"), "utf8"));
  const optional = {};

  for (const t of TARGETS) {
    const archive = join(distDir, `ajiro-api-${version}-${t.target}.${t.ext}`);
    if (!existsSync(archive)) {
      if (allowMissing) {
        console.warn(`skip ${t.target}: ${archive} not found`);
        continue;
      }
      throw new Error(`missing archive: ${archive}`);
    }
    const name = `@niti-lab/ajiro-api-${t.platform}-${t.arch}`;
    const pkgDir = join(outDir, `cli-${t.platform}-${t.arch}`);
    const tmp = mkdtempSync(join(tmpdir(), "ajiro-api-"));
    extract(archive, tmp, t.ext);

    const exe = t.platform === "win32" ? "ajiro-api.exe" : "ajiro-api";
    const binary = findFile(tmp, exe);
    if (!binary) throw new Error(`${exe} not found in ${archive}`);
    mkdirSync(join(pkgDir, "bin"), { recursive: true });
    cpSync(binary, join(pkgDir, "bin", exe));
    if (t.platform !== "win32") chmodSync(join(pkgDir, "bin", exe), 0o755);
    cpSync(join(root, "LICENSE"), join(pkgDir, "LICENSE"));
    writeFileSync(
      join(pkgDir, "README.md"),
      `# ${name}\n\nNative ajiro-api binary for ${t.platform}/${t.arch}. Do not install directly; use \`npx ajiro-api\`.\n\nhttps://github.com/niti-lab/ajiro-api\n`
    );
    writeFileSync(
      join(pkgDir, "package.json"),
      JSON.stringify(
        {
          name,
          version,
          description: `ajiro-api native binary for ${t.platform}/${t.arch}`,
          license: "MIT",
          homepage: "https://github.com/niti-lab/ajiro-api",
          repository: { type: "git", url: "git+https://github.com/niti-lab/ajiro-api.git" },
          os: t.os,
          cpu: t.cpu,
          files: ["bin/", "README.md", "LICENSE"],
          preferUnplugged: true,
        },
        null,
        2
      ) + "\n"
    );
    optional[name] = version;
    rmSync(tmp, { recursive: true, force: true });
    console.log(`built ${name}@${version}`);
  }

  // Main package: copy, pin version and optional dependencies.
  const mainOut = join(outDir, "ajiro-api");
  cpSync(join(root, "npm", "ajiro-api"), mainOut, { recursive: true });
  cpSync(join(root, "LICENSE"), join(mainOut, "LICENSE"));
  mainPkg.version = version;
  mainPkg.optionalDependencies = optional;
  writeFileSync(join(mainOut, "package.json"), JSON.stringify(mainPkg, null, 2) + "\n");
  chmodSync(join(mainOut, "bin", "ajiro-api.js"), 0o755);
  console.log(`built ajiro-api@${version} with ${Object.keys(optional).length} platform package(s)`);
}

main();
