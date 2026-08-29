import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import sharp from "sharp";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const source = path.join(root, "apps", "web", "public", "favicon.svg");
const outputDir = path.join(root, "apps", "desktop", "build");
const pngPath = path.join(outputDir, "icon.png");
const icoPath = path.join(outputDir, "icon.ico");
const sizes = [16, 24, 32, 48, 64, 128, 256];

function createIco(images) {
  const headerSize = 6 + images.length * 16;
  const totalSize = headerSize + images.reduce((sum, image) => sum + image.data.length, 0);
  const ico = Buffer.alloc(totalSize);
  ico.writeUInt16LE(0, 0);
  ico.writeUInt16LE(1, 2);
  ico.writeUInt16LE(images.length, 4);

  let offset = headerSize;
  images.forEach(({ size, data }, index) => {
    const entry = 6 + index * 16;
    ico.writeUInt8(size === 256 ? 0 : size, entry);
    ico.writeUInt8(size === 256 ? 0 : size, entry + 1);
    ico.writeUInt8(0, entry + 2);
    ico.writeUInt8(0, entry + 3);
    ico.writeUInt16LE(1, entry + 4);
    ico.writeUInt16LE(32, entry + 6);
    ico.writeUInt32LE(data.length, entry + 8);
    ico.writeUInt32LE(offset, entry + 12);
    data.copy(ico, offset);
    offset += data.length;
  });
  return ico;
}

fs.mkdirSync(outputDir, { recursive: true });
const images = [];
for (const size of sizes) {
  const data = await sharp(source)
    .resize(size, size, { fit: "contain" })
    .png()
    .toBuffer();
  images.push({ size, data });
}
fs.writeFileSync(pngPath, images.at(-1).data);
fs.writeFileSync(icoPath, createIco(images));
console.log(`[desktop-package] generated ${path.relative(root, icoPath)}`);
