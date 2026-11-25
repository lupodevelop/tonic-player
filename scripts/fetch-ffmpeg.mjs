import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const pins = JSON.parse(readFileSync(join(root, "scripts", "ffmpeg.json"), "utf8"));
const args = process.argv.slice(2);
const ifMissing = args.includes("--if-missing");
// The Tauri CLI sets TAURI_ENV_TARGET_TRIPLE for its before-commands.
const triple =
  args.find((a) => !a.startsWith("--")) ||
  process.env.TAURI_ENV_TARGET_TRIPLE ||
  /^host: (.+)$/m.exec(execFileSync("rustc", ["-vV"], { encoding: "utf8" }))[1];

const pin = pins[triple];
if (!pin) {
  console.error(`No FFmpeg build pinned for ${triple}; known: ${Object.keys(pins).join(", ")}`);
  process.exit(1);
}

const exe = triple.includes("windows") ? ".exe" : "";
const outDir = join(root, "src-tauri", "binaries");
const outputs = Object.keys(pin.files).map((name) => join(outDir, `${name}-${triple}${exe}`));
if (ifMissing && outputs.every((p) => existsSync(p))) process.exit(0);

async function get(url) {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
  return Buffer.from(await res.arrayBuffer());
}

// A file without a pinned sha256 is checked against the SHA256SUMS published
// next to it; the hash printed here is what goes into ffmpeg.json.
const sums = new Map();
async function expected(url) {
  const base = url.slice(0, url.lastIndexOf("/") + 1);
  if (!sums.has(base)) {
    const text = (await get(`${base}SHA256SUMS`)).toString("utf8");
    sums.set(base, new Map(text.trim().split("\n").map((l) => l.split(/\s+\*?/).reverse())));
  }
  const sum = sums.get(base).get(url.slice(base.length));
  if (!sum) throw new Error(`${url}: not listed in ${base}SHA256SUMS`);
  return sum;
}

mkdirSync(outDir, { recursive: true });
for (const [name, file] of Object.entries(pin.files)) {
  console.log(`Fetching ${file.url}`);
  const bytes = await get(file.url);
  const sum = createHash("sha256").update(bytes).digest("hex");
  const want = file.sha256 || (await expected(file.url));
  if (sum !== want) throw new Error(`${file.url}: sha256 ${sum}, expected ${want}`);
  const out = join(outDir, `${name}-${triple}${exe}`);
  writeFileSync(out, bytes);
  chmodSync(out, 0o755);
  console.log(`Wrote ${out} (sha256 ${sum}${file.sha256 ? "" : ", not pinned in scripts/ffmpeg.json"})`);
}
