import { createHash } from "node:crypto";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { basename, extname, join, resolve } from "node:path";
import { performance } from "node:perf_hooks";

const MIB = 1024 * 1024;
/** @typedef {{ start: number, end: number, hash: string }} Chunk */
const SUPPORTED = new Set([".jpg", ".jpeg", ".png", ".webp", ".dng", ".raw", ".psd", ".mp4", ".mov", ".mkv"]);
const GEAR = Array.from({ length: 256 }, (_, value) => {
  let x = BigInt(value + 1) * 0x9e3779b97f4a7c15n;
  x ^= x >> 30n;
  x *= 0xbf58476d1ce4e5b9n;
  x ^= x >> 27n;
  return BigInt.asUintN(64, x * 0x94d049bb133111ebn ^ (x >> 31n));
});

/** @param {string[]} argv */
function parseArgs(argv) {
  const roots = /** @type {string[]} */ ([]);
  let output = ".tmp/media-object-storage-bench/report.json";
  for (let index = 0; index < argv.length; index += 1) {
    if (argv[index] === "--corpus" && argv[index + 1]) roots.push(resolve(argv[++index]));
    else if (argv[index] === "--output" && argv[index + 1]) output = resolve(argv[++index]);
    else if (!argv[index].startsWith("-") && output === ".tmp/media-object-storage-bench/report.json") output = resolve(argv[index]);
    else throw new Error(`unknown or incomplete argument: ${argv[index]}`);
  }
  return { roots, output };
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

/** @param {string[]} roots */
async function collectFiles(roots) {
  const files = /** @type {string[]} */ ([]);
  async function walk(directory) {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) await walk(path);
      else if (entry.isFile() && SUPPORTED.has(extname(entry.name).toLowerCase())) files.push(path);
    }
  }
  for (const root of roots) await walk(root);
  return files.sort();
}

function syntheticCorpus() {
  const compressible = Buffer.alloc(24 * MIB);
  for (let index = 0; index < compressible.length; index += 4096) {
    compressible.writeUInt32LE(index >>> 0, index);
  }
  const incompressible = Buffer.alloc(24 * MIB);
  let state = 0x6d2b79f5;
  for (let index = 0; index < incompressible.length; index += 1) {
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    incompressible[index] = state & 0xff;
  }
  const shaped = [
    ["synthetic-jpeg", Buffer.from([0xff, 0xd8, 0xff, 0xe1])],
    ["synthetic-png", Buffer.from("89504e470d0a1a0a", "hex")],
    ["synthetic-webp", Buffer.from("RIFF0000WEBP")],
    ["synthetic-dng", Buffer.from("49492a0008000000", "hex")],
    ["synthetic-psd", Buffer.from("8BPS\u0000\u0001")],
    ["synthetic-mp4", Buffer.from("000000186674797069736f6d", "hex")],
  ].map(([kind, header], ordinal) => {
    const bytes = Buffer.alloc(256 * 1024, ordinal * 31 + 7);
    header.copy(bytes);
    return { kind, bytes };
  });
  return [
    { kind: "synthetic-compressible", bytes: compressible },
    { kind: "synthetic-incompressible", bytes: incompressible },
    ...shaped,
  ];
}

function fixedChunks(bytes, target = 8 * MIB) {
  const chunks = /** @type {Chunk[]} */ ([]);
  for (let start = 0; start < bytes.length; start += target) {
    const end = Math.min(bytes.length, start + target);
    chunks.push({ start, end, hash: sha256(bytes.subarray(start, end)) });
  }
  return chunks;
}

function cdcChunks(bytes, min = 2 * MIB, target = 8 * MIB, max = 16 * MIB) {
  if (bytes.length === 0) return [];
  if (bytes.length <= min) return [{ start: 0, end: bytes.length, hash: sha256(bytes) }];
  const bits = Math.round(Math.log2(target));
  const strictMask = (1n << BigInt(bits + 1)) - 1n;
  const looseMask = (1n << BigInt(bits - 1)) - 1n;
  const chunks = /** @type {Chunk[]} */ ([]);
  let start = 0;
  let rolling = 0n;
  const windowSize = 63;
  const window = new Uint8Array(windowSize);
  let windowIndex = 0;
  let windowFilled = 0;
  for (let index = 0; index < bytes.length; index += 1) {
    rolling = BigInt.asUintN(64, (rolling << 1n) | (rolling >> 63n));
    if (windowFilled === windowSize) rolling ^= BigInt.asUintN(64, (GEAR[window[windowIndex]] << BigInt(windowSize)) | (GEAR[window[windowIndex]] >> BigInt(64 - windowSize)));
    else windowFilled += 1;
    rolling ^= GEAR[bytes[index]];
    window[windowIndex] = bytes[index];
    windowIndex = (windowIndex + 1) % windowSize;
    const length = index + 1 - start;
    const mask = length < target ? strictMask : looseMask;
    if (length >= min && ((rolling & mask) === 0n || length >= max)) {
      const end = index + 1;
      chunks.push({ start, end, hash: sha256(bytes.subarray(start, end)) });
      start = end;
    }
  }
  if (start < bytes.length) chunks.push({ start, end: bytes.length, hash: sha256(bytes.subarray(start)) });
  return chunks;
}

function mutations(bytes) {
  const marker = Buffer.from("MEGLE-LOCALIZED-EDIT-V1");
  const header = Buffer.from(bytes);
  marker.copy(header, 0, 0, Math.min(marker.length, header.length));
  const middle = Math.floor(bytes.length / 2);
  return {
    exact: bytes,
    header,
    insert_start: Buffer.concat([marker, bytes]),
    insert_middle: Buffer.concat([bytes.subarray(0, middle), marker, bytes.subarray(middle)]),
    append: Buffer.concat([bytes, marker]),
    truncate: bytes.subarray(0, Math.max(0, bytes.length - Math.min(MIB, bytes.length))),
    transcode: Buffer.from(bytes).reverse(),
  };
}

function reuseBytes(base, edited) {
  const reusable = new Map(base.map((chunk) => [chunk.hash, chunk.end - chunk.start]));
  return edited.reduce((total, chunk) => total + (reusable.get(chunk.hash) ?? 0), 0);
}

/** @param {Chunk[]} chunks */
function simulatePacking(chunks, target) {
  let packs = 0;
  let current = 0;
  let currentChunkCount = 0;
  let firstSeal = 0;
  let wholeObjectBytesForSingleChunkReads = 0;
  for (const chunk of chunks) {
    const length = chunk.end - chunk.start;
    if (current > 0 && current + length > target) {
      packs += 1;
      if (firstSeal === 0) firstSeal = current;
      wholeObjectBytesForSingleChunkReads += current * currentChunkCount;
      current = 0;
      currentChunkCount = 0;
    }
    current += length;
    currentChunkCount += 1;
  }
  if (current > 0) {
    packs += 1;
    if (firstSeal === 0) firstSeal = current;
    wholeObjectBytesForSingleChunkReads += current * currentChunkCount;
  }
  const inputBytes = chunks.reduce((sum, chunk) => sum + chunk.end - chunk.start, 0);
  return {
    target_mib: target / MIB,
    packs,
    first_seal_bytes: firstSeal,
    index_bytes: chunks.length * 48,
    estimated_peak_temp_bytes: Math.min(
      target,
      inputBytes,
    ),
    bounded_range_amplification: 1,
    whole_object_single_chunk_amplification:
      inputBytes === 0 ? 1 : wholeObjectBytesForSingleChunkReads / inputBytes,
  };
}

async function main() {
  const { roots, output } = parseArgs(process.argv.slice(2));
  const corpus = syntheticCorpus();
  for (const path of await collectFiles(roots)) {
    corpus.push({ kind: `local-${extname(path).slice(1).toLowerCase() || "media"}`, bytes: await readFile(path) });
  }
  const profiles = [
    { name: "fixed-8mib", chunk: fixedChunks },
    { name: "cdc-2-8-16mib", chunk: cdcChunks },
  ];
  const started = performance.now();
  const results = /** @type {Array<Record<string, unknown>>} */ ([]);
  for (const profile of profiles) {
    const profileStart = performance.now();
    const cpuStart = process.cpuUsage();
    let inputBytes = 0;
    let chunkCount = 0;
    let reusedBytes = 0;
    let editedBytes = 0;
    const reuseByEdit = new Map();
    const allChunks = /** @type {Chunk[]} */ ([]);
    for (const item of corpus) {
      const base = profile.chunk(item.bytes);
      inputBytes += item.bytes.length;
      chunkCount += base.length;
      allChunks.push(...base);
      for (const [edit, edited] of Object.entries(mutations(item.bytes))) {
        const chunks = profile.chunk(edited);
        const reused = reuseBytes(base, chunks);
        reusedBytes += reused;
        editedBytes += edited.length;
        const aggregate = reuseByEdit.get(edit) ?? { reused: 0, bytes: 0 };
        aggregate.reused += reused;
        aggregate.bytes += edited.length;
        reuseByEdit.set(edit, aggregate);
      }
    }
    const elapsedMs = performance.now() - profileStart;
    const cpuUsage = process.cpuUsage(cpuStart);
    const cpuMs = (cpuUsage.user + cpuUsage.system) / 1000;
    const packSimulation = [32, 64, 96, 128].map((size) => {
      const simulation = simulatePacking(allChunks, size * MIB);
      return {
        ...simulation,
        estimated_time_to_first_seal_ms: Number(
          ((simulation.first_seal_bytes / inputBytes) * elapsedMs).toFixed(2),
        ),
      };
    });
    results.push({
      profile: profile.name,
      input_bytes: inputBytes,
      chunk_count: chunkCount,
      manifest_estimate_bytes: chunkCount * 192,
      edit_reuse_ratio: editedBytes === 0 ? 1 : reusedBytes / editedBytes,
      edit_reuse_ratio_by_case: Object.fromEntries(
        [...reuseByEdit].map(([edit, value]) => [
          edit,
          value.bytes === 0 ? 1 : value.reused / value.bytes,
        ]),
      ),
      elapsed_ms: Number(elapsedMs.toFixed(2)),
      cpu_ms: Number(cpuMs.toFixed(2)),
      cpu_core_utilization: Number((cpuMs / elapsedMs).toFixed(3)),
      throughput_mib_s: Number((inputBytes / MIB / (elapsedMs / 1000)).toFixed(2)),
      pack_simulation: packSimulation,
    });
  }
  const report = {
    schema_version: 1,
    command: "npm run bench:media-objects -- [--corpus <directory>] [--output <ignored-json>]",
    corpus: {
      item_count: corpus.length,
      kinds: [...new Set(corpus.map((item) => item.kind))].sort(),
      aggregate_sha256: sha256(Buffer.concat(corpus.map((item) => Buffer.from(sha256(item.bytes), "hex")))),
      total_bytes: corpus.reduce((sum, item) => sum + item.bytes.length, 0),
    },
    peak_rss_bytes: process.memoryUsage().rss,
    total_elapsed_ms: Number((performance.now() - started).toFixed(2)),
    profiles: results,
    selected: {
      chunking: "cdc-2-8-16mib",
      small_file_threshold_bytes: 2 * MIB,
      data_pack_target_bytes: 64 * MIB,
      data_pack_seal_min_bytes: 32 * MIB,
      data_pack_seal_max_bytes: 96 * MIB,
      manifest_pack_target_bytes: 4 * MIB,
      manifest_pack_max_bytes: 8 * MIB,
      rationale: "CDC restores stable boundaries after localized insertions; 64 MiB balances object count, retry granularity, and bounded temporary disk.",
    },
  };
  await mkdir(resolve(output, ".."), { recursive: true });
  await writeFile(output, `${JSON.stringify(report, null, 2)}\n`);
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
}

await main();
