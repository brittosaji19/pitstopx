#!/usr/bin/env node
// Dependency-free fallback: emit a 1024x1024 RGBA PNG of the PitStopX gauge
// motif (coral ring on a dark rounded square) for `tauri icon` input, used
// when no SVG rasterizer (sharp/rsvg/inkscape) is available.
import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";

const N = 1024;
const buf = Buffer.alloc(N * N * 4);

const cx = N / 2,
  cy = N / 2;
const rOuter = N * 0.40,
  rInner = N * 0.30;

function set(x, y, r, g, b, a) {
  const i = (y * N + x) * 4;
  buf[i] = r;
  buf[i + 1] = g;
  buf[i + 2] = b;
  buf[i + 3] = a;
}

for (let y = 0; y < N; y++) {
  for (let x = 0; x < N; x++) {
    // Rounded-square dark background.
    const inset = N * 0.06;
    const inSquare =
      x > inset && x < N - inset && y > inset && y < N - inset;
    if (inSquare) set(x, y, 0x22, 0x22, 0x25, 0xff);

    // Coral gauge ring.
    const d = Math.hypot(x - cx, y - cy);
    if (d <= rOuter && d >= rInner) set(x, y, 0xd9, 0x77, 0x57, 0xff);
    // Hub.
    if (d <= N * 0.05) set(x, y, 0xd9, 0x77, 0x57, 0xff);
  }
}

// Assemble PNG.
function crc32(bytes) {
  let c = ~0;
  for (let i = 0; i < bytes.length; i++) {
    c ^= bytes[i];
    for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
  }
  return ~c >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const typeBuf = Buffer.from(type, "ascii");
  const body = Buffer.concat([typeBuf, data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(N, 0);
ihdr.writeUInt32BE(N, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // RGBA
// Filtered scanlines (filter byte 0 per row).
const raw = Buffer.alloc(N * (N * 4 + 1));
for (let y = 0; y < N; y++) {
  raw[y * (N * 4 + 1)] = 0;
  buf.copy(raw, y * (N * 4 + 1) + 1, y * N * 4, (y + 1) * N * 4);
}
const png = Buffer.concat([
  Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw)),
  chunk("IEND", Buffer.alloc(0)),
]);

const out = process.argv[2] || "scripts/source-icon.png";
writeFileSync(out, png);
console.log(`Wrote ${out} (${png.length} bytes)`);
