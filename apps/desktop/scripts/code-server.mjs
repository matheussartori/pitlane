#!/usr/bin/env node
// Puts code-server (open source VS Code by Coder, MIT) into src-tauri/resources as one archive,
// for the build to bundle; the app unpacks it on the first use of the Code tab. The version is
// pinned in code-server.version (the first run pins the latest release). Runs before `tauri dev`
// and `tauri build`; does nothing when the pinned archive is there.
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const pinFile = join(here, "..", "code-server.version");
const res = join(here, "..", "src-tauri", "resources");
const arch = { arm64: "arm64", x64: "amd64" }[process.arch];
if (process.platform !== "darwin" || !arch) {
  console.error(`code-server: no macOS build for ${process.platform}/${process.arch}`);
  process.exit(1);
}

let version = existsSync(pinFile) ? readFileSync(pinFile, "utf8").trim() : "";
if (!version) {
  const rel = JSON.parse(execFileSync("curl", ["-fsSL", "-H", "Accept: application/vnd.github+json", "https://api.github.com/repos/coder/code-server/releases/latest"], { encoding: "utf8" }));
  version = rel.tag_name.replace(/^v/, "");
  writeFileSync(pinFile, version + "\n");
  console.log(`code-server: pinned ${version}`);
}

// Icons with Cursor's calm: Fluent Icons (the workbench) and Symbols (files), MIT, from Open VSX.
const extensions = [
  ["miguelsolorio", "fluent-icons", "0.0.19"],
  ["miguelsolorio", "symbols", "0.0.26"],
];
const extDir = join(res, "vscode-extensions");
mkdirSync(extDir, { recursive: true });
for (const [ns, name, ver] of extensions) {
  const file = join(extDir, `${ns}.${name}-${ver}.vsix`);
  if (existsSync(file)) continue;
  console.log(`code-server: fetching ${ns}.${name} ${ver}`);
  execFileSync("curl", ["-fsSL", "--retry", "2", "-o", `${file}.part`, `https://open-vsx.org/api/${ns}/${name}/${ver}/file/${ns}.${name}-${ver}.vsix`], { stdio: "inherit" });
  renameSync(`${file}.part`, file);
}

const archive = join(res, "code-server.tar.gz");
const stamp = join(res, "code-server.version");
const want = `${version}-macos-${arch}`;
if (existsSync(archive) && existsSync(stamp) && readFileSync(stamp, "utf8").trim() === want) process.exit(0);

mkdirSync(res, { recursive: true });
const url = `https://github.com/coder/code-server/releases/download/v${version}/code-server-${want}.tar.gz`;
console.log(`code-server: downloading ${want}`);
execFileSync("curl", ["-fL", "--retry", "2", "-o", `${archive}.part`, url], { stdio: "inherit" });
renameSync(`${archive}.part`, archive);
writeFileSync(stamp, `${want}\n`);
console.log(`code-server: ${want} ready to bundle`);
