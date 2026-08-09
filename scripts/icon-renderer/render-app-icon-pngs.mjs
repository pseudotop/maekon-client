import { mkdir, stat } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import sharp from "sharp";

const [sourceSvg, outputDir, traySourceSvg] = process.argv.slice(2);

if (!sourceSvg || !outputDir) {
  console.error("Usage: render-app-icon-pngs.mjs <source-svg> <output-dir> [tray-source-svg]");
  process.exit(2);
}

async function isFile(filePath) {
  if (!filePath) return false;
  try {
    return (await stat(filePath)).isFile();
  } catch {
    return false;
  }
}

async function render(source, filename, size) {
  await sharp(source, { density: 384 })
    .resize(size, size, { fit: "contain" })
    .png({ compressionLevel: 9, adaptiveFiltering: false, palette: false })
    .toFile(path.join(outputDir, filename));
}

if (!(await isFile(sourceSvg))) {
  console.error(`Source logo not found: ${sourceSvg}`);
  process.exit(1);
}

await mkdir(outputDir, { recursive: true });

for (const [filename, size] of [
  ["32x32.png", 32],
  ["128x128.png", 128],
  ["128x128@2x.png", 256],
  ["icon.png", 1024],
  ["dock_icon.png", 1024],
]) {
  await render(sourceSvg, filename, size);
}

if (await isFile(traySourceSvg)) {
  await render(traySourceSvg, "tray_icon.png", 22);
  await render(traySourceSvg, "tray_icon@2x.png", 44);
}
