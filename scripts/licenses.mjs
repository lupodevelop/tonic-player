import { execSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const run = (cmd) => execSync(cmd, { cwd: root, encoding: "utf8", maxBuffer: 256 << 20 });
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
const link = (url, text = url) => `<a href="${esc(url)}" target="_blank" rel="noreferrer">${esc(text)}</a>`;

const FSR_LICENSE = `Copyright (c) 2021 Advanced Micro Devices, Inc. All rights reserved.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.`;

// The triple picks the FFmpeg build, as in fetch-ffmpeg.mjs.
const triple =
  process.argv[2] || process.env.TAURI_ENV_TARGET_TRIPLE || /^host: (.+)$/m.exec(run("rustc -vV"))[1];
const ffmpeg = JSON.parse(readFileSync(join(root, "scripts", "ffmpeg.json"), "utf8"))[triple];
if (!ffmpeg) throw new Error(`No FFmpeg build pinned for ${triple}`);

const app = JSON.parse(readFileSync(join(root, "package.json"), "utf8")).name;
const rust = JSON.parse(run("cargo about generate --format json --manifest-path src-tauri/Cargo.toml"));
const crates = rust.licenses
  .map((l) => ({ ...l, used_by: l.used_by.filter((u) => u.crate.name !== app) }))
  .filter((l) => l.used_by.length);

const npm = Object.values(JSON.parse(run("pnpm licenses list --prod --json"))).flat();
// The licence file shipped in each package, when it has one.
const npmText = (dir) => {
  const file = existsSync(dir) && readdirSync(dir).find((f) => /^(licen[cs]e|copying)(\.|$)/i.test(f));
  return file ? readFileSync(join(dir, file), "utf8") : "";
};

const section = (title, items) => {
  if (!items.length) throw new Error(`No entries for ${title}`);
  return `<section><h2>${esc(title)}</h2>${items.join("\n")}</section>`;
};

const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Third-party licences</title>
<style>
  :root { color-scheme: dark; font: 13px/1.45 -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif; }
  body { margin: 0; padding: 4px 16px 24px; background: transparent; color: rgb(255 255 255 / 0.92); }
  h2 { margin: 18px 0 6px; font-size: 13px; color: rgb(255 255 255 / 0.5); }
  h3 { margin: 12px 0 2px; font-size: 13px; font-weight: 600; }
  p { margin: 2px 0; }
  .muted { color: rgb(255 255 255 / 0.5); }
  a { color: inherit; }
  details { margin: 4px 0 10px; }
  summary { cursor: pointer; color: rgb(255 255 255 / 0.5); }
  pre { white-space: pre-wrap; font-size: 11px; color: rgb(255 255 255 / 0.7); }
</style>
</head>
<body>
${section("FFmpeg", [
  `<h3>FFmpeg ${esc(ffmpeg.version)}</h3>
<p>${esc(ffmpeg.licence)} · build: ${link(ffmpeg.build)} · source: ${link(ffmpeg.source)}</p>
<p class="muted">Bundled as separate programs (ffmpeg, ffprobe). The build page lists the libraries compiled in.</p>`,
])}
${section("Shaders", [
  `<h3>AMD FidelityFX Super Resolution 1.0 (EASU, RCAS)</h3>
<p class="muted">MIT · ported to WebGL for upscaling · ${link("https://github.com/GPUOpen-Effects/FidelityFX-FSR")}</p>
<details><summary>Licence text</summary><pre>${esc(FSR_LICENSE)}</pre></details>`,
])}
${section(
  "Rust crates",
  crates.map(
    (l) => `<h3>${l.used_by.map((u) => esc(`${u.crate.name} ${u.crate.version}`)).join(", ")}</h3>
<p class="muted">${esc(l.name)}</p>
<details><summary>Licence text</summary><pre>${esc(l.text)}</pre></details>`,
  ),
)}
${section(
  "JavaScript packages",
  npm.map((p) => {
    const text = npmText(p.paths[0]);
    return `<h3>${esc(p.name)} ${esc(p.versions.join(", "))}</h3>
<p class="muted">${esc(p.license)}${p.homepage ? ` · ${link(p.homepage)}` : ""}</p>
${text ? `<details><summary>Licence text</summary><pre>${esc(text)}</pre></details>` : ""}`;
  }),
)}
</body>
</html>
`;

mkdirSync(join(root, "public"), { recursive: true });
writeFileSync(join(root, "public", "licenses.html"), html);
console.log(`Wrote public/licenses.html (FFmpeg ${ffmpeg.version}, ${crates.length} crate licences, ${npm.length} npm packages)`);
