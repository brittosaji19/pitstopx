#!/usr/bin/env node
/**
 * Regenerate the app icon set from the single vector source (`scripts/icon.svg`).
 *
 * Tauri's `tauri icon` command takes one square PNG (>= 1024px recommended) and
 * emits every size/format the bundler needs (.icns, .ico, PNG set) into
 * `src-tauri/icons/`. This wrapper rasterizes the SVG to a temp PNG first
 * (using `sharp` when available, else `rsvg-convert`/`inkscape`), then invokes
 * `tauri icon`.
 *
 * Usage:  node scripts/make-icon.mjs
 */
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const svg = join(root, "scripts", "icon.svg");
const out = join(mkdtempSync(join(tmpdir(), "pitstopx-icon-")), "icon.png");

if (!existsSync(svg)) {
  console.error(`Missing ${svg}`);
  process.exit(1);
}

async function rasterize() {
  // Preferred: sharp (pure-Node, no system deps).
  try {
    const sharp = (await import("sharp")).default;
    await sharp(svg, { density: 384 }).resize(1024, 1024).png().toFile(out);
    return;
  } catch {
    /* fall through to CLI rasterizers */
  }
  for (const [cmd, args] of [
    ["rsvg-convert", ["-w", "1024", "-h", "1024", svg, "-o", out]],
    ["inkscape", [svg, "--export-type=png", "-w", "1024", "-h", "1024", "-o", out]],
  ]) {
    try {
      execFileSync(cmd, args, { stdio: "inherit" });
      return;
    } catch {
      /* try the next one */
    }
  }
  throw new Error(
    "No rasterizer found. Install `sharp` (npm i -D sharp) or rsvg-convert/inkscape."
  );
}

await rasterize();
console.log(`Rasterized -> ${out}`);

// Hand off to the Tauri icon generator.
execFileSync("npx", ["tauri", "icon", out], { stdio: "inherit", cwd: root });
console.log("Icons written to src-tauri/icons/");
