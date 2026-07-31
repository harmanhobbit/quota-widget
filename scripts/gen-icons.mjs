// Generates the app icons (PNG + ICO) with no dependencies beyond node:zlib.
// Design: rounded-square badge with a gauge arc — matches the widget's purpose.
import { deflateSync } from 'node:zlib';
import { writeFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const outDir = join(dirname(fileURLToPath(import.meta.url)), '..', 'src-tauri', 'icons');
mkdirSync(outDir, { recursive: true });

// ---- minimal PNG encoder ----------------------------------------------------

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

function encodePng(size, rgba) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  // filter type 0 per scanline
  const raw = Buffer.alloc(size * (size * 4 + 1));
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0;
    rgba.copy(raw, y * (size * 4 + 1) + 1, y * size * 4, (y + 1) * size * 4);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

// ---- icon drawing -----------------------------------------------------------

function drawIcon(size) {
  const rgba = Buffer.alloc(size * size * 4);
  const c = (size - 1) / 2;
  const radius = size * 0.46;
  const corner = size * 0.22;
  const half = size * 0.44;
  const bg = [0x25, 0x2b, 0x3a]; // slate badge
  const arcColor = [0x4a, 0xda, 0x7c]; // green gauge
  const arcInner = size * 0.26;
  const arcOuter = size * 0.38;
  // gauge sweeps 225° → -45° (bottom-left to bottom-right); fill first 70%.
  const start = (5 * Math.PI) / 4;
  const sweep = (3 * Math.PI) / 2;

  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const dx = x - c;
      const dy = y - c;
      // rounded-square coverage
      const qx = Math.max(Math.abs(dx) - (half - corner), 0);
      const qy = Math.max(Math.abs(dy) - (half - corner), 0);
      const distSq = Math.hypot(qx, qy) - corner;
      const cov = Math.min(Math.max(0.5 - distSq, 0), 1);
      if (cov === 0) continue;
      let [r, g, b] = bg;

      const rad = Math.hypot(dx, dy);
      if (rad >= arcInner && rad <= arcOuter) {
        // angle measured clockwise from the start of the gauge
        let ang = Math.atan2(-dy, dx); // math coords, y up
        let rel = (start - ang + Math.PI * 2) % (Math.PI * 2);
        if (rel <= sweep) {
          const lit = rel <= sweep * 0.7;
          const track = [0x3a, 0x44, 0x58];
          [r, g, b] = lit ? arcColor : track;
        }
      }
      // needle dot in the middle
      if (rad < size * 0.06) [r, g, b] = [0xe8, 0xec, 0xf4];

      const i = (y * size + x) * 4;
      rgba[i] = r;
      rgba[i + 1] = g;
      rgba[i + 2] = b;
      rgba[i + 3] = Math.round(cov * 255);
    }
  }
  return rgba;
}

// ---- ICO container (PNG-compressed entries, Vista+) -------------------------

function encodeIco(pngsBySize) {
  const entries = [...pngsBySize.entries()];
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2); // type: icon
  header.writeUInt16LE(entries.length, 4);
  let offset = 6 + entries.length * 16;
  const dirs = [];
  const blobs = [];
  for (const [size, png] of entries) {
    const dir = Buffer.alloc(16);
    dir[0] = size >= 256 ? 0 : size;
    dir[1] = size >= 256 ? 0 : size;
    dir.writeUInt16LE(1, 4); // planes
    dir.writeUInt16LE(32, 6); // bpp
    dir.writeUInt32LE(png.length, 8);
    dir.writeUInt32LE(offset, 12);
    offset += png.length;
    dirs.push(dir);
    blobs.push(png);
  }
  return Buffer.concat([header, ...dirs, ...blobs]);
}

// ---- write everything -------------------------------------------------------

const pngs = new Map();
for (const size of [16, 32, 48, 128, 256]) {
  pngs.set(size, encodePng(size, drawIcon(size)));
}
writeFileSync(join(outDir, '32x32.png'), pngs.get(32));
writeFileSync(join(outDir, '128x128.png'), pngs.get(128));
writeFileSync(join(outDir, '128x128@2x.png'), pngs.get(256));
writeFileSync(join(outDir, 'icon.png'), pngs.get(256));
writeFileSync(
  join(outDir, 'icon.ico'),
  encodeIco(new Map([16, 32, 48, 256].map((s) => [s, pngs.get(s)])))
);
console.log(`wrote icons to ${outDir}`);
