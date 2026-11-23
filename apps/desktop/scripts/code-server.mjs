#!/usr/bin/env node
// Puts code-server (open source VS Code by Coder, MIT) into src-tauri/resources as one archive,
// for the build to bundle; the app unpacks it on the first use of the Code tab. The version is
// pinned in code-server.version (the first run pins the latest release). Runs before `tauri dev`
// and `tauri build`; does nothing when the pinned archive is there.
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, renameSync, rmSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
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

// Pitlane's own bridge (apps/desktop/vscode-bridge): "Ask Claude about this" in the editor. Packed
// as a .vsix named by its version, so a new version installs over the old one.
{
  const src = join(here, "..", "vscode-bridge");
  const pkg = JSON.parse(readFileSync(join(src, "package.json"), "utf8"));
  const id = `${pkg.publisher}.${pkg.name}`;
  const file = join(extDir, `${id}-${pkg.version}.vsix`);
  for (const f of readdirSync(extDir)) if (f.startsWith(`${id}-`) && f.endsWith(".vsix") && join(extDir, f) !== file) unlinkSync(join(extDir, f));
  if (!existsSync(file)) {
    const tmp = mkdtempSync(join(tmpdir(), "pitlane-bridge-"));
    mkdirSync(join(tmp, "extension"));
    for (const f of ["package.json", "extension.js"]) cpSync(join(src, f), join(tmp, "extension", f));
    writeFileSync(
      join(tmp, "[Content_Types].xml"),
      `<?xml version="1.0" encoding="utf-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension=".json" ContentType="application/json"/><Default Extension=".js" ContentType="application/javascript"/><Default Extension=".vsixmanifest" ContentType="text/xml"/></Types>`,
    );
    writeFileSync(
      join(tmp, "extension.vsixmanifest"),
      `<?xml version="1.0" encoding="utf-8"?>
<PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011" xmlns:d="http://schemas.microsoft.com/developer/vsx-schema-design/2011">
  <Metadata>
    <Identity Language="en-US" Id="${pkg.name}" Version="${pkg.version}" Publisher="${pkg.publisher}" />
    <DisplayName>${pkg.displayName}</DisplayName>
    <Description xml:space="preserve">${pkg.description}</Description>
    <Categories>Other</Categories>
    <Properties><Property Id="Microsoft.VisualStudio.Code.Engine" Value="${pkg.engines.vscode}" /></Properties>
  </Metadata>
  <Installation><InstallationTarget Id="Microsoft.VisualStudio.Code" /></Installation>
  <Dependencies />
  <Assets><Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" Addressable="true" /></Assets>
</PackageManifest>
`,
    );
    execFileSync("zip", ["-qrX", "-nw", `${file}.part`, "[Content_Types].xml", "extension.vsixmanifest", "extension"], { cwd: tmp });
    renameSync(`${file}.part`, file);
    rmSync(tmp, { recursive: true, force: true });
    console.log(`code-server: packed ${id} ${pkg.version}`);
  }
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
