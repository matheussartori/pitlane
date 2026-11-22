#!/usr/bin/env node
// Puts the Whisper model (large-v3-turbo, q5_0, MIT, from ggerganov/whisper.cpp on Hugging Face)
// into src-tauri/resources/whisper for the build to bundle: dictation runs on this Mac, offline,
// from the first use. Runs before `tauri dev` and `tauri build`; does nothing when the file is there.
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, renameSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const name = "ggml-large-v3-turbo-q5_0.bin";
const size = 574041195; // bytes, as published; a short file is a broken download
const dir = join(dirname(fileURLToPath(import.meta.url)), "..", "src-tauri", "resources", "whisper");
const file = join(dir, name);
if (existsSync(file) && statSync(file).size === size) process.exit(0);
mkdirSync(dir, { recursive: true });
console.log(`whisper: fetching ${name} (550 MB, once)`);
execFileSync("curl", ["-fL", "--retry", "3", "-C", "-", "-o", `${file}.part`, `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/${name}`], { stdio: "inherit" });
const got = statSync(`${file}.part`).size;
if (got !== size) {
  console.error(`whisper: ${name} is ${got} bytes, expected ${size}; run again to resume`);
  process.exit(1);
}
renameSync(`${file}.part`, file);
