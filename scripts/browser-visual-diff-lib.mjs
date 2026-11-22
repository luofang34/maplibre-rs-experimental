import { posix, resolve, sep } from 'node:path';
import { deflateSync, inflateSync } from 'node:zlib';

const PNG_SIGNATURE = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) {
      c = (c & 1) !== 0 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    }
    table[n] = c >>> 0;
  }
  return table;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) {
    c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  }
  return (c ^ 0xffffffff) >>> 0;
}

function paethPredictor(a, b, c) {
  const p = a + b - c;
  const pa = Math.abs(p - a);
  const pb = Math.abs(p - b);
  const pc = Math.abs(p - c);
  if (pa <= pb && pa <= pc) return a;
  if (pb <= pc) return b;
  return c;
}

export function decodePng(buffer) {
  const buf = Buffer.isBuffer(buffer) ? buffer : Buffer.from(buffer);
  if (buf.length < 8 || !buf.subarray(0, 8).equals(PNG_SIGNATURE)) {
    throw new Error('Invalid PNG signature');
  }

  let offset = 8;
  let width = 0;
  let height = 0;
  let bitDepth = 0;
  let colorType = 0;
  let interlace = 0;
  let sawIhdr = false;
  let sawIend = false;
  const idatChunks = [];

  while (offset + 12 <= buf.length) {
    const length = buf.readUInt32BE(offset);
    const type = buf.toString('ascii', offset + 4, offset + 8);
    const dataStart = offset + 8;
    const dataEnd = dataStart + length;
    if (dataEnd + 4 > buf.length) {
      throw new Error(`Truncated PNG chunk ${type}`);
    }
    const data = buf.subarray(dataStart, dataEnd);
    offset = dataEnd + 4;

    if (type === 'IHDR') {
      if (length !== 13) throw new Error('Invalid IHDR length');
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      bitDepth = data[8];
      colorType = data[9];
      interlace = data[12];
      sawIhdr = true;
    } else if (type === 'IDAT') {
      idatChunks.push(data);
    } else if (type === 'IEND') {
      sawIend = true;
      break;
    }
  }

  if (!sawIhdr || !sawIend || width === 0 || height === 0) {
    throw new Error('PNG missing required IHDR/IEND or has zero dimensions');
  }
  if (bitDepth !== 8) {
    throw new Error(`Unsupported PNG bit depth ${bitDepth}; expected 8`);
  }
  if (interlace !== 0) {
    throw new Error(`Unsupported interlaced PNG (method ${interlace})`);
  }

  let bpp;
  if (colorType === 6) bpp = 4;
  else if (colorType === 2) bpp = 3;
  else throw new Error(`Unsupported PNG colorType ${colorType}; expected RGB (2) or RGBA (6)`);

  const raw = inflateSync(Buffer.concat(idatChunks));
  const stride = width * bpp;
  const expectedRaw = height * (stride + 1);
  if (raw.length < expectedRaw) {
    throw new Error(`Decompressed PNG length ${raw.length} < expected ${expectedRaw}`);
  }

  const reconstructed = new Uint8Array(height * stride);
  for (let y = 0; y < height; y++) {
    const inRow = y * (stride + 1);
    const filter = raw[inRow];
    const outRow = y * stride;
    const prevRow = (y - 1) * stride;

    for (let x = 0; x < stride; x++) {
      const byte = raw[inRow + 1 + x];
      const left = x >= bpp ? reconstructed[outRow + x - bpp] : 0;
      const up = y > 0 ? reconstructed[prevRow + x] : 0;
      const upLeft = y > 0 && x >= bpp ? reconstructed[prevRow + x - bpp] : 0;

      let val;
      switch (filter) {
        case 0:
          val = byte;
          break;
        case 1:
          val = (byte + left) & 0xff;
          break;
        case 2:
          val = (byte + up) & 0xff;
          break;
        case 3:
          val = (byte + ((left + up) >>> 1)) & 0xff;
          break;
        case 4:
          val = (byte + paethPredictor(left, up, upLeft)) & 0xff;
          break;
        default:
          throw new Error(`Unsupported PNG scanline filter ${filter}`);
      }
      reconstructed[outRow + x] = val;
    }
  }

  if (bpp === 4) {
    return { width, height, rgba: reconstructed };
  }
  const rgba = new Uint8Array(width * height * 4);
  for (let i = 0, j = 0; i < reconstructed.length; i += 3, j += 4) {
    rgba[j] = reconstructed[i];
    rgba[j + 1] = reconstructed[i + 1];
    rgba[j + 2] = reconstructed[i + 2];
    rgba[j + 3] = 255;
  }
  return { width, height, rgba };
}

function pngChunk(type, payload) {
  const chunk = Buffer.alloc(12 + payload.length);
  chunk.writeUInt32BE(payload.length, 0);
  chunk.write(type, 4, 4, 'ascii');
  payload.copy(chunk, 8);
  chunk.writeUInt32BE(crc32(chunk.subarray(4, 8 + payload.length)), 8 + payload.length);
  return chunk;
}

export function encodeRgbaPng(width, height, rgba) {
  if (rgba.length !== width * height * 4) {
    throw new Error(`Expected ${width * height * 4} RGBA bytes, got ${rgba.length}`);
  }
  const stride = width * 4;
  const raw = Buffer.alloc(height * (stride + 1));
  for (let y = 0; y < height; y++) {
    const outRow = y * (stride + 1);
    raw[outRow] = 0;
    const srcStart = y * stride;
    raw.set(rgba.subarray(srcStart, srcStart + stride), outRow + 1);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  ihdr[10] = 0;
  ihdr[11] = 0;
  ihdr[12] = 0;

  return Buffer.concat([
    PNG_SIGNATURE,
    pngChunk('IHDR', ihdr),
    pngChunk('IDAT', deflateSync(raw)),
    pngChunk('IEND', Buffer.alloc(0)),
  ]);
}

export function compareRgbaBuffers(a, b, width, height, { channelThreshold = 8 } = {}) {
  const totalPixels = width * height;
  if (a.length !== totalPixels * 4 || b.length !== totalPixels * 4) {
    throw new Error(
      `Buffer dimension mismatch for ${width}x${height}: got ${a.length} and ${b.length}`,
    );
  }
  let diffPixels = 0;
  let maxChannelDiff = 0;
  let sumAbsDiff = 0;
  const diffRgba = new Uint8Array(totalPixels * 4);

  for (let i = 0; i < totalPixels; i++) {
    const off = i * 4;
    const dr = Math.abs(a[off] - b[off]);
    const dg = Math.abs(a[off + 1] - b[off + 1]);
    const db = Math.abs(a[off + 2] - b[off + 2]);
    const da = Math.abs(a[off + 3] - b[off + 3]);
    const pxMax = Math.max(dr, dg, db, da);
    sumAbsDiff += dr + dg + db + da;
    if (pxMax > maxChannelDiff) {
      maxChannelDiff = pxMax;
    }
    if (pxMax > channelThreshold) {
      diffPixels++;
      diffRgba[off] = 255;
      diffRgba[off + 1] = 32;
      diffRgba[off + 2] = 64;
      diffRgba[off + 3] = 255;
    } else {
      const luma = Math.round((a[off] * 299 + a[off + 1] * 587 + a[off + 2] * 114) / 3000);
      diffRgba[off] = luma;
      diffRgba[off + 1] = luma;
      diffRgba[off + 2] = luma;
      diffRgba[off + 3] = 255;
    }
  }

  return {
    totalPixels,
    diffPixels,
    diffRatio: totalPixels > 0 ? diffPixels / totalPixels : 0,
    maxChannelDiff,
    meanAbsDiff: totalPixels > 0 ? sumAbsDiff / (totalPixels * 4) : 0,
    diffRgba,
  };
}

export function analyzeImageContent(rgba, width, height) {
  const totalPixels = width * height;
  if (rgba.length !== totalPixels * 4) {
    throw new Error(`Expected ${totalPixels * 4} bytes, got ${rgba.length}`);
  }
  let minR = 255;
  let maxR = 0;
  let minG = 255;
  let maxG = 0;
  let minB = 255;
  let maxB = 0;
  const distinct = new Set();
  const stride = Math.max(1, Math.floor(totalPixels / 4096));

  for (let i = 0; i < totalPixels; i += stride) {
    const off = i * 4;
    const r = rgba[off];
    const g = rgba[off + 1];
    const b = rgba[off + 2];
    if (r < minR) minR = r;
    if (r > maxR) maxR = r;
    if (g < minG) minG = g;
    if (g > maxG) maxG = g;
    if (b < minB) minB = b;
    if (b > maxB) maxB = b;
    if (distinct.size < 256) {
      distinct.add((r << 16) | (g << 8) | b);
    }
  }

  const maxChannelSpan = Math.max(maxR - minR, maxG - minG, maxB - minB);
  return {
    totalPixels,
    maxChannelSpan,
    distinctSampleColors: distinct.size,
    isUniform: maxChannelSpan < 4 || distinct.size <= 1,
  };
}

export function resolveBundleAssetPath(bundleDir, requestUrl) {
  const cleanUrl = requestUrl.split('#')[0].split('?')[0];
  let decoded;
  try {
    decoded = decodeURIComponent(cleanUrl);
  } catch {
    return null;
  }
  const relPosix = posix.normalize(decoded.replace(/^\/+/, ''));
  if (!relPosix || relPosix === '.' || relPosix === '..' || relPosix.startsWith('../')) {
    return null;
  }
  const rootAbs = resolve(bundleDir);
  const candidate = resolve(rootAbs, ...relPosix.split('/'));
  if (candidate !== rootAbs && !candidate.startsWith(rootAbs + sep)) {
    return null;
  }
  return candidate;
}

export function formatDiffReport(summary) {
  const lines = [
    '# Browser WebGPU / WebGL2 Differential Visual Report',
    '',
    `Generated: ${summary.generatedAt}`,
    `Max allowed diff ratio: ${(summary.maxDiffRatio * 100).toFixed(2)}% (channel threshold ${summary.channelThreshold})`,
    '',
    '## Captures',
    '| Browser | Scene | Requested | Active Backend | Distinct Colors | Span | Status |',
    '| :--- | :--- | :--- | :--- | ---: | ---: | :--- |',
  ];
  for (const cap of summary.captures) {
    const span = cap.content ? cap.content.maxChannelSpan : '-';
    const colors = cap.content ? cap.content.distinctSampleColors : '-';
    lines.push(
      `| ${cap.browser} | ${cap.scene} | ${cap.backend} | ${cap.activeBackend ?? '-'} | ${colors} | ${span} | ${cap.status} |`,
    );
  }
  lines.push(
    '',
    '## Pairwise Diffs',
    '| Comparison | Scene | Diff Pixels | Diff % | Max Diff | Mean Abs | Status |',
    '| :--- | :--- | ---: | ---: | ---: | ---: | :--- |',
  );
  for (const cmp of summary.comparisons) {
    lines.push(
      `| ${cmp.label} | ${cmp.scene} | ${cmp.diffPixels}/${cmp.totalPixels} | ${(cmp.diffRatio * 100).toFixed(3)}% | ${cmp.maxChannelDiff} | ${cmp.meanAbsDiff.toFixed(2)} | ${cmp.ok ? 'PASS' : 'FAIL'} |`,
    );
  }
  const skipped = summary.captures.filter((cap) => cap.error);
  if (skipped.length > 0) {
    lines.push('', '## Skipped Captures');
    for (const cap of skipped) {
      lines.push(`- \`${cap.browser}\` (${cap.scene} / ${cap.backend}): ${cap.status} — ${cap.error}`);
    }
  }
  return `${lines.join('\n')}\n`;
}
