/**
 * Pure logic of the web app that has no test runner of its own (apps/web is checked with
 * tsc only): the QR encoder. The test decodes the matrix back independently — format bits,
 * unmasking, the zigzag, de-interleaving, Reed–Solomon syndromes and the byte stream — so a
 * mistake anywhere in the encoder shows up without a camera.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { qrMatrix, qrSvgPath } from '../../web/src/lib/qr.ts';

// EC level M: [EC codewords per block, [blocks, data codewords]…] for versions 1–15 (ISO/IEC 18004, table 9)
const BLOCKS: [number, [number, number][]][] = [
  [10, [[1, 16]]], [16, [[1, 28]]], [26, [[1, 44]]], [18, [[2, 32]]], [24, [[2, 43]]], [16, [[4, 27]]], [18, [[4, 31]]],
  [22, [[2, 38], [2, 39]]], [22, [[3, 36], [2, 37]]], [26, [[4, 43], [1, 44]]], [30, [[1, 50], [4, 51]]], [22, [[6, 36], [2, 37]]],
  [22, [[8, 37], [1, 38]]], [24, [[4, 40], [5, 41]]], [24, [[5, 41], [5, 42]]],
];
const capacity = (v: number) => BLOCKS[v - 1]![1].reduce((s, [n, d]) => s + n * d, 0);
/** The longest byte text of a version: mode (4 bits) + length (8 or 16) + data. */
const maxBytes = (v: number) => Math.floor((capacity(v) * 8 - 4 - (v < 10 ? 8 : 16)) / 8);

// GF(256) with the QR polynomial, by log tables (the encoder multiplies bit by bit)
const EXP: number[] = [];
const LOG: number[] = new Array(256).fill(0);
for (let i = 0, x = 1; i < 255; i++) {
  EXP[i] = x;
  LOG[x] = i;
  x <<= 1;
  if (x & 0x100) x ^= 0x11d;
}
const gmul = (a: number, b: number) => (a && b ? EXP[(LOG[a]! + LOG[b]!) % 255]! : 0);
/** A block is a valid Reed–Solomon codeword when every syndrome is zero. */
function syndromesZero(block: number[], ecLen: number): boolean {
  for (let i = 0; i < ecLen; i++) {
    let s = 0;
    for (const b of block) s = gmul(s, EXP[i]!) ^ b;
    if (s !== 0) return false;
  }
  return true;
}

const MASKS = [
  (x: number, y: number) => (x + y) % 2 === 0,
  (_x: number, y: number) => y % 2 === 0,
  (x: number) => x % 3 === 0,
  (x: number, y: number) => (x + y) % 3 === 0,
  (x: number, y: number) => (Math.floor(x / 3) + Math.floor(y / 2)) % 2 === 0,
  (x: number, y: number) => ((x * y) % 2) + ((x * y) % 3) === 0,
  (x: number, y: number) => (((x * y) % 2) + ((x * y) % 3)) % 2 === 0,
  (x: number, y: number) => (((x + y) % 2) + ((x * y) % 3)) % 2 === 0,
];

/** Alignment pattern centres of a version (ISO/IEC 18004, annex E). */
const ALIGN: number[][] = [[], [6, 18], [6, 22], [6, 26], [6, 30], [6, 34], [6, 22, 38], [6, 24, 42], [6, 26, 46], [6, 28, 50], [6, 30, 54], [6, 32, 58], [6, 34, 62], [6, 26, 46, 66], [6, 26, 48, 70]];

/** Which modules are function patterns (not data). */
function functionMap(version: number): boolean[][] {
  const size = version * 4 + 17;
  const f: boolean[][] = Array.from({ length: size }, () => new Array<boolean>(size).fill(false));
  const mark = (x0: number, y0: number, w: number, h: number) => {
    for (let y = y0; y < y0 + h; y++) for (let x = x0; x < x0 + w; x++) if (x >= 0 && y >= 0 && x < size && y < size) f[y]![x] = true;
  };
  mark(0, 0, 9, 9);
  mark(size - 8, 0, 8, 9);
  mark(0, size - 8, 9, 8);
  mark(6, 0, 1, size);
  mark(0, 6, size, 1);
  const pos = ALIGN[version - 1]!;
  for (const ax of pos)
    for (const ay of pos) {
      if ((ax === 6 && ay === 6) || (ax === 6 && ay === size - 7) || (ax === size - 7 && ay === 6)) continue;
      mark(ax - 2, ay - 2, 5, 5);
    }
  if (version >= 7) {
    mark(size - 11, 0, 3, 6);
    mark(0, size - 11, 6, 3);
  }
  return f;
}

/** Decode a clean matrix back to its text (byte mode, level M). */
function decode(m: boolean[][]): { text: string; version: number; mask: number } {
  const size = m.length;
  const version = (size - 17) / 4;
  assert.ok(Number.isInteger(version) && version >= 1 && version <= 15, `a QR size, got ${size}`);
  assert.ok(m.every((row) => row.length === size), 'square');

  // format information: 15 bits around the top-left finder, BCH(15,5), XOR 0x5412
  const at = (x: number, y: number) => (m[y]![x] ? 1 : 0);
  let fmt = 0;
  for (let i = 0; i <= 5; i++) fmt |= at(8, i) << i;
  fmt |= at(8, 7) << 6;
  fmt |= at(8, 8) << 7;
  fmt |= at(7, 8) << 8;
  for (let i = 9; i < 15; i++) fmt |= at(14 - i, 8) << i;
  let copy = 0;
  for (let i = 0; i < 8; i++) copy |= at(size - 1 - i, 8) << i;
  for (let i = 8; i < 15; i++) copy |= at(8, size - 15 + i) << i;
  assert.equal(copy, fmt, 'both copies of the format information agree');
  assert.equal(at(8, size - 8), 1, 'the dark module');
  const raw = fmt ^ 0x5412;
  const data5 = raw >>> 10;
  let rem = data5;
  for (let i = 0; i < 10; i++) rem = (rem << 1) ^ ((rem >>> 9) * 0x537);
  assert.equal(raw & 0x3ff, rem & 0x3ff, 'format BCH');
  assert.equal(data5 >>> 3, 0, 'error correction level M');
  const mask = data5 & 7;

  if (version >= 7) {
    let v = 0;
    for (let i = 0; i < 18; i++) v |= at(size - 11 + (i % 3), Math.floor(i / 3)) << i;
    let v2 = 0;
    for (let i = 0; i < 18; i++) v2 |= at(Math.floor(i / 3), size - 11 + (i % 3)) << i;
    assert.equal(v, v2, 'both copies of the version information agree');
    assert.equal(v >>> 12, version, 'version information');
    let r = version;
    for (let i = 0; i < 12; i++) r = (r << 1) ^ ((r >>> 11) * 0x1f25);
    assert.equal(v & 0xfff, r & 0xfff, 'version BCH');
  }

  // timing patterns and finders
  for (let i = 8; i < size - 8; i++) {
    assert.equal(m[6]![i], i % 2 === 0, 'horizontal timing');
    assert.equal(m[i]![6], i % 2 === 0, 'vertical timing');
  }
  for (const [cx, cy] of [[3, 3], [size - 4, 3], [3, size - 4]] as const) {
    for (let d = -3; d <= 3; d++) {
      assert.ok(m[cy - 3]![cx + d] && m[cy + 3]![cx + d] && m[cy + d]![cx - 3] && m[cy + d]![cx + 3], 'finder ring');
    }
    assert.ok(m[cy]![cx] && !m[cy - 2]![cx] && !m[cy]![cx + 2], 'finder centre and gap');
  }

  // the data bits in the zigzag, unmasked
  const fn = functionMap(version);
  const bits: number[] = [];
  for (let right = size - 1; right >= 1; right -= 2) {
    if (right === 6) right = 5;
    for (let vert = 0; vert < size; vert++)
      for (let j = 0; j < 2; j++) {
        const x = right - j;
        const upward = ((right + 1) & 2) === 0;
        const y = upward ? size - 1 - vert : vert;
        if (!fn[y]![x]) bits.push(at(x, y) ^ (MASKS[mask]!(x, y) ? 1 : 0));
      }
  }
  const [ecLen, groups] = BLOCKS[version - 1]!;
  const lens = groups.flatMap(([n, d]) => new Array<number>(n).fill(d));
  const total = lens.reduce((a, b) => a + b, 0) + ecLen * lens.length;
  assert.ok(bits.length >= total * 8 && bits.length < total * 8 + 8, `${bits.length} data modules for ${total} codewords`);
  const words: number[] = [];
  for (let i = 0; i < total; i++) words.push(bits.slice(i * 8, i * 8 + 8).reduce((a, b) => (a << 1) | b, 0));

  // de-interleave: data column by column over the blocks, then EC the same way
  const blocks = lens.map(() => ({ d: [] as number[], e: [] as number[] }));
  let k = 0;
  for (let i = 0; i < Math.max(...lens); i++) lens.forEach((len, b) => i < len && blocks[b]!.d.push(words[k++]!));
  for (let i = 0; i < ecLen; i++) blocks.forEach((b) => b.e.push(words[k++]!));
  assert.equal(k, total);
  for (const b of blocks) assert.ok(syndromesZero([...b.d, ...b.e], ecLen), 'a valid Reed–Solomon block');

  // the stream: mode 0100, the length, the bytes, a terminator, padding 0xEC 0x11
  const stream = blocks.flatMap((b) => b.d).flatMap((w) => Array.from({ length: 8 }, (_, i) => (w >>> (7 - i)) & 1));
  let p = 0;
  const take = (n: number) => {
    let v = 0;
    for (let i = 0; i < n; i++) v = (v << 1) | stream[p++]!;
    return v;
  };
  assert.equal(take(4), 4, 'byte mode');
  const len = take(version < 10 ? 8 : 16);
  const bytes = Uint8Array.from({ length: len }, () => take(8));
  const restBits = stream.length - p;
  assert.equal(take(Math.min(4, restBits)), 0, 'terminator');
  p += (8 - (p % 8)) % 8;
  for (let i = 0; p + 8 <= stream.length; i++) assert.equal(take(8), i % 2 === 0 ? 0xec : 0x11, 'padding codewords');
  return { text: new TextDecoder('utf-8', { fatal: true }).decode(bytes), version, mask };
}

test('QR: short texts decode back, in the smallest version that fits', () => {
  for (const text of ['hi', 'http://192.168.1.20:8790/', 'http://my-hub.example.com:8795/settings/profile?tab=security#x', 'Привет, мир — 你好 👋', '']) {
    const got = decode(qrMatrix(text));
    assert.equal(got.text, text);
    const bytes = new TextEncoder().encode(text).length;
    assert.ok(bytes <= maxBytes(got.version), 'fits');
    if (got.version > 1) assert.ok(bytes > maxBytes(got.version - 1), `the smallest version, not ${got.version}`);
  }
});

test('QR: every version 1–15 at its exact capacity and one byte over', () => {
  for (let v = 1; v <= 15; v++) {
    const full = 'a'.repeat(maxBytes(v));
    const got = decode(qrMatrix(full));
    assert.equal(got.version, v, `${full.length} bytes fill version ${v}`);
    assert.equal(got.text, full);
    if (v < 15) assert.equal(decode(qrMatrix(full + 'b')).version, v + 1, 'one more byte needs the next version');
  }
  // 9 → 10: the length field grows from 8 to 16 bits
  assert.equal(maxBytes(9), 180);
  assert.equal(maxBytes(10), 213);
  assert.throws(() => qrMatrix('a'.repeat(maxBytes(15) + 1)), /too long/i);
});

test('QR: the mask with the lowest penalty is a valid one, different texts differ', () => {
  const masks = new Set<number>();
  for (let i = 0; i < 40; i++) masks.add(decode(qrMatrix(`http://10.0.0.${i}:87${10 + i}/page/${'x'.repeat(i)}`)).mask);
  assert.ok(masks.size >= 3, `several masks get chosen across inputs, got ${[...masks]}`);
  assert.notDeepEqual(qrMatrix('a'), qrMatrix('b'));
  assert.deepEqual(qrMatrix('same'), qrMatrix('same'), 'deterministic');
});

test('QR: the SVG path has one square per dark module inside a 4-module quiet zone', () => {
  const m = qrMatrix('http://localhost:8790/');
  const { d, size } = qrSvgPath('http://localhost:8790/');
  assert.equal(size, m.length + 8);
  const squares = [...d.matchAll(/M(\d+) (\d+)h1v1h-1z/g)];
  assert.equal(squares.length, m.flat().filter(Boolean).length);
  assert.equal(squares.map((s) => s[0]).join(''), d, 'nothing else in the path');
  for (const s of squares) {
    const x = Number(s[1]);
    const y = Number(s[2]);
    assert.ok(x >= 4 && y >= 4 && x < size - 4 && y < size - 4, 'inside the quiet zone');
    assert.equal(m[y - 4]![x - 4], true);
  }
});

test('QR: a real scanner reads it (when zbarimg is installed)', (t) => {
  try {
    execFileSync('zbarimg', ['--version'], { stdio: 'ignore' });
  } catch {
    return t.skip('zbarimg is not installed');
  }
  const dir = mkdtempSync(join(tmpdir(), 'hubness-qr-'));
  for (const [i, text] of ['http://192.168.1.20:8790/cart?x=1', 'a'.repeat(maxBytes(9)), 'a'.repeat(maxBytes(9) + 1), 'Привет '.repeat(20)].entries()) {
    const m = qrMatrix(text);
    const scale = 6;
    const quiet = 4;
    const n = (m.length + 2 * quiet) * scale;
    const px = Buffer.alloc(n * n, 255);
    m.forEach((row, y) =>
      row.forEach((dark, x) => {
        if (dark) for (let a = 0; a < scale; a++) px.fill(0, ((y + quiet) * scale + a) * n + (x + quiet) * scale, ((y + quiet) * scale + a) * n + (x + quiet + 1) * scale);
      }),
    );
    const file = join(dir, `${i}.pgm`);
    writeFileSync(file, Buffer.concat([Buffer.from(`P5 ${n} ${n} 255\n`), px]));
    assert.equal(execFileSync('zbarimg', ['-q', '--raw', file], { encoding: 'utf8' }).replace(/\n$/, ''), text);
  }
});

test('web: an effect written without braces returns nothing React could take for a cleanup', () => {
  // `useEffect(() => el.scrollIntoView())` looks harmless, but the effect returns whatever the
  // call returns — browsers now return a Promise there, and React calls the previous return
  // as a cleanup function on the next update: the whole app goes blank. Without braces only a
  // state setter (returns undefined) or something that is known to return a cleanup is allowed.
  const root = new URL('../../web/src/', import.meta.url).pathname;
  const allowed = [/^set[A-Z]\w*\(/, /^installLeaderKeys\(\)/, /^\(\) =>/];
  const bad: string[] = [];
  for (const name of readdirSync(root, { recursive: true }) as string[]) {
    if (!/\.tsx?$/.test(name)) continue;
    readFileSync(join(root, name), 'utf8')
      .split('\n')
      .forEach((line, i) => {
        const m = /use(?:Layout)?Effect\(\(\) => (?!\{)(.*)/.exec(line);
        if (m && !allowed.some((ok) => ok.test(m[1]!))) bad.push(`${name}:${i + 1}: ${line.trim()}`);
      });
  }
  assert.deepEqual(bad, []);
});
