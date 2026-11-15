import { MAX_ATTACHMENT_BASE64 } from "../shared/model.ts";

export interface CompressedImage {
  data: string;
  mime: string;
  width: number;
  height: number;
  name: string;
}

function blobToBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const value = String(reader.result ?? "");
      resolve(value.slice(value.indexOf(",") + 1));
    };
    reader.onerror = () => reject(reader.error ?? new Error("读取图片失败"));
    reader.readAsDataURL(blob);
  });
}

function canvasBlob(canvas: HTMLCanvasElement, quality: number): Promise<Blob | null> {
  return new Promise((resolve) => canvas.toBlob(resolve, "image/webp", quality));
}

/**
 * Downscale and WebP-encode a pasted / dropped / picked image until it fits
 * one synchronized field (MAX_ATTACHMENT_BASE64). Notes images are
 * illustrations of a task, not archival copies, so fidelity yields to sync.
 */
export async function compressImage(source: Blob, name = "图片"): Promise<CompressedImage> {
  if (!source.type.startsWith("image/")) throw new Error("只能添加图片");
  const bitmap = await createImageBitmap(source);
  try {
    let maxSide = 1600;
    let quality = 0.82;
    for (let attempt = 0; attempt < 10; attempt += 1) {
      const scale = Math.min(1, maxSide / Math.max(bitmap.width, bitmap.height));
      const width = Math.max(1, Math.round(bitmap.width * scale));
      const height = Math.max(1, Math.round(bitmap.height * scale));
      const canvas = document.createElement("canvas");
      canvas.width = width;
      canvas.height = height;
      const context = canvas.getContext("2d");
      if (!context) throw new Error("无法处理图片");
      context.drawImage(bitmap, 0, 0, width, height);
      const blob = await canvasBlob(canvas, quality);
      if (!blob) throw new Error("无法编码图片");
      const data = await blobToBase64(blob);
      if (data.length <= MAX_ATTACHMENT_BASE64) return { data, mime: "image/webp", width, height, name: name.slice(0, 120) };
      if (quality > 0.56) quality -= 0.13;
      else maxSide = Math.round(maxSide * 0.72);
    }
    throw new Error("图片过大，无法添加");
  } finally {
    bitmap.close();
  }
}

export function attachmentUrl(attachment: { mime: string; data: string }): string {
  return `data:${attachment.mime};base64,${attachment.data}`;
}

/** Image files of a paste / drop, in order. */
export function imageFiles(data: DataTransfer | null): File[] {
  if (!data) return [];
  return Array.from(data.files).filter((file) => file.type.startsWith("image/"));
}
