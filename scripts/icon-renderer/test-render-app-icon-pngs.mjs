import assert from "node:assert/strict";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { spawnSync } from "node:child_process";
import sharp from "sharp";

const rendererDir = path.dirname(new URL(import.meta.url).pathname);
const clientRoot = path.resolve(rendererDir, "../..");
const sourceSvg = path.join(clientRoot, "assets/brand/logo-icon.svg");
const traySourceSvg = path.join(clientRoot, "assets/brand/tray-template.svg");
const renderer = path.join(rendererDir, "render-app-icon-pngs.mjs");
const firstOutput = await mkdtemp(path.join(os.tmpdir(), "maekon-icon-first-"));
const secondOutput = await mkdtemp(path.join(os.tmpdir(), "maekon-icon-second-"));

const expected = new Map([
  ["32x32.png", 32],
  ["128x128.png", 128],
  ["128x128@2x.png", 256],
  ["icon.png", 1024],
  ["dock_icon.png", 1024],
  ["tray_icon.png", 22],
  ["tray_icon@2x.png", 44],
]);

function runRenderer(outputDir) {
  const result = spawnSync(process.execPath, [renderer, sourceSvg, outputDir, traySourceSvg], {
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stderr);
}

function hasExactColor(raw, color) {
  for (let index = 0; index < raw.length; index += 4) {
    if (
      raw[index] === color[0] &&
      raw[index + 1] === color[1] &&
      raw[index + 2] === color[2] &&
      raw[index + 3] > 0
    ) {
      return true;
    }
  }
  return false;
}

try {
  runRenderer(firstOutput);
  runRenderer(secondOutput);

  for (const [filename, size] of expected) {
    const firstPath = path.join(firstOutput, filename);
    const secondPath = path.join(secondOutput, filename);
    const metadata = await sharp(firstPath).metadata();
    assert.equal(metadata.width, size, `${filename} width`);
    assert.equal(metadata.height, size, `${filename} height`);
    assert.deepEqual(await readFile(firstPath), await readFile(secondPath), `${filename} is deterministic`);
  }

  for (const filename of ["icon.png", "dock_icon.png"]) {
    const { data, info } = await sharp(path.join(firstOutput, filename))
      .ensureAlpha()
      .raw()
      .toBuffer({ resolveWithObject: true });
    assert.equal(info.channels, 4);
    assert(hasExactColor(data, [43, 22, 120]), `${filename} preserves the inner border color`);
    assert(hasExactColor(data, [162, 140, 255]), `${filename} preserves the smile color`);
  }

  console.log("[OK] Sharp icon renderer preserves brand details and is deterministic.");
} finally {
  await rm(firstOutput, { recursive: true, force: true });
  await rm(secondOutput, { recursive: true, force: true });
}
