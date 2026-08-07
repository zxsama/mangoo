import { createHash } from "node:crypto";
import { access, mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const FIXTURE_KIND = "megle-task8-controlled-media-v1";
const FIXTURES = [
  { relativePath: "landscape-sample.bmp", width: 1600, height: 900, seed: 17 },
  { relativePath: "portrait-sample.bmp", width: 900, height: 1600, seed: 43 },
  {
    relativePath: path.join("img2img-images", "nested-sample.bmp"),
    width: 72,
    height: 72,
    seed: 79
  },
  {
    relativePath: path.join("txt2img-images", "txt2img-sample.bmp"),
    width: 320,
    height: 180,
    seed: 101
  },
  ...Array.from({ length: 6 }, (_, index) => ({
    relativePath: path.join("img2img-grids", `grid-sample-${String(index + 1).padStart(2, "0")}.bmp`),
    width: 320,
    height: 180,
    seed: 137 + index * 13
  }))
];

function sha256(buffer) {
  return createHash("sha256").update(buffer).digest("hex");
}

function createBitmap({ width, height, seed }) {
  const rowStride = Math.ceil((width * 3) / 4) * 4;
  const pixelBytes = rowStride * height;
  const buffer = Buffer.alloc(54 + pixelBytes);
  buffer.write("BM", 0, 2, "ascii");
  buffer.writeUInt32LE(buffer.length, 2);
  buffer.writeUInt32LE(54, 10);
  buffer.writeUInt32LE(40, 14);
  buffer.writeInt32LE(width, 18);
  buffer.writeInt32LE(height, 22);
  buffer.writeUInt16LE(1, 26);
  buffer.writeUInt16LE(24, 28);
  buffer.writeUInt32LE(pixelBytes, 34);
  buffer.writeInt32LE(2835, 38);
  buffer.writeInt32LE(2835, 42);

  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      const offset = 54 + y * rowStride + x * 3;
      buffer[offset] = (seed + x * 5 + y * 3) % 256;
      buffer[offset + 1] = (seed * 3 + x * 2 + y * 7) % 256;
      buffer[offset + 2] = (seed * 7 + x * 11 + y) % 256;
    }
  }
  return buffer;
}

function expectedManifest(root) {
  return {
    kind: FIXTURE_KIND,
    root,
    files: FIXTURES.map((fixture) => {
      const buffer = createBitmap(fixture);
      return {
        relativePath: fixture.relativePath,
        width: fixture.width,
        height: fixture.height,
        byteLength: buffer.length,
        sha256: sha256(buffer)
      };
    })
  };
}

export async function resolveRepositoryRoot(harnessUrl) {
  const harnessPath = fileURLToPath(harnessUrl);
  const candidate = path.resolve(path.dirname(harnessPath), "..", "..");
  try {
    const packageJson = JSON.parse(await readFile(path.join(candidate, "package.json"), "utf8"));
    if (packageJson.name !== "megle") {
      throw new Error(`package name is ${JSON.stringify(packageJson.name)}`);
    }
    await Promise.all([
      access(path.join(candidate, "apps", "desktop", "src", "main.ts")),
      access(path.join(candidate, "apps", "web", "src", "app", "App.tsx"))
    ]);
  } catch (error) {
    throw new Error(
      `Task8 harness could not validate repository root ${candidate} resolved from ${harnessPath}: ${error}`
    );
  }
  return candidate;
}

export async function validateControlledMediaRoot(root) {
  const expected = expectedManifest(root);
  try {
    if (path.basename(root).toLowerCase() !== "outputs") {
      throw new Error(`fixture root must be named outputs, received ${path.basename(root)}`);
    }
    const marker = JSON.parse(await readFile(path.join(root, "controlled-fixture.json"), "utf8"));
    if (marker.kind !== FIXTURE_KIND) {
      throw new Error(`unexpected fixture marker ${JSON.stringify(marker.kind)}`);
    }
    for (let index = 0; index < FIXTURES.length; index += 1) {
      const fixture = FIXTURES[index];
      const expectedFile = expected.files[index];
      const file = await readFile(path.join(root, fixture.relativePath));
      if (file.subarray(0, 2).toString("ascii") !== "BM") {
        throw new Error(`${fixture.relativePath} is not a BMP`);
      }
      if (file.readInt32LE(18) !== fixture.width || file.readInt32LE(22) !== fixture.height) {
        throw new Error(`${fixture.relativePath} dimensions do not match the controlled fixture`);
      }
      if (file.length !== expectedFile.byteLength || sha256(file) !== expectedFile.sha256) {
        throw new Error(`${fixture.relativePath} bytes do not match the controlled fixture`);
      }
    }
  } catch (error) {
    throw new Error(`Controlled media fixture validation failed for ${root}: ${error}`);
  }
  return expected;
}

export async function prepareControlledMediaRoot(visualRoot, runId) {
  const root = path.join(visualRoot, "runtime-media", runId, "outputs");
  await mkdir(root, { recursive: true });
  for (const fixture of FIXTURES) {
    const target = path.join(root, fixture.relativePath);
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, createBitmap(fixture));
  }
  await writeFile(
    path.join(root, "controlled-fixture.json"),
    `${JSON.stringify({ kind: FIXTURE_KIND, files: FIXTURES.map(({ relativePath, width, height }) => ({
      relativePath,
      width,
      height
    })) }, null, 2)}\n`
  );
  return validateControlledMediaRoot(root);
}
