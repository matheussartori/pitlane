#!/usr/bin/env node
// Builds the `pit` CLI (which also runs Pitlane's daemon) and puts it where Tauri bundles external
// binaries: src-tauri/binaries/pit-<target>. The app runs the copy beside its own binary, so an
// installed Pitlane needs nothing on the PATH. `--release` for `tauri build`, debug for `tauri dev`.
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, chmodSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..", "..", "..");
const release = process.argv.includes("--release");
const env = { ...process.env, PATH: `/opt/homebrew/opt/rustup/bin:${process.env.HOME}/.cargo/bin:${process.env.PATH}` };
const triple = execFileSync("rustc", ["-vV"], { encoding: "utf8", env }).match(/host: (\S+)/)[1];
execFileSync("cargo", ["build", "-q", "-p", "pit", ...(release ? ["--release"] : [])], { cwd: root, stdio: "inherit", env });
const out = join(here, "..", "src-tauri", "binaries");
mkdirSync(out, { recursive: true });
const dest = join(out, `pit-${triple}`);
copyFileSync(join(root, "target", release ? "release" : "debug", "pit"), dest);
chmodSync(dest, 0o755);
console.log(`sidecar: pit (${release ? "release" : "debug"}) -> ${dest}`);
