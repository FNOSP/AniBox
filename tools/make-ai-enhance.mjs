#!/usr/bin/env node
/* [lc-1820] AI 增强组件打包：把已就位的 ai-enhance 目录（RIFE Vulkan 运行时）打成
   R2 上传物（zip + manifest.json 清单）。产物放 release/ai-enhance/（不入库）。
   用法：node tools/make-ai-enhance.mjs [版本号]（默认按目录内 manifest 的 version）*/
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { execFileSync } from "node:child_process";

const SRC = path.join(process.env.APPDATA, "com.anibox.app", "ai-enhance");
const OUT_DIR = path.resolve("release", "ai-enhance");
const REQUIRED = [
  "vs/rife.vpy",
  "vs/superres.vpy",
  "vs/superres_trt.vpy",
  "runtime/Lib/site-packages/vapoursynth/vsscript.dll",
  "runtime/libvapoursynth.dll",
  "runtime/plugins/librife.dll",
  "runtime/plugins/MiscFilters.dll",
  "runtime/plugins/vsort.dll",
  "runtime/plugins/vsort/onnxruntime.dll",
  "runtime/plugins/vsort/DirectML.dll",
  "runtime/Lib/site-packages/vsmlrt.py",
  "runtime/models/rife-v4.6/flownet.bin",
  "runtime/models/rife-v4.6/flownet.param",
  "runtime/models/animejanai/animejanai-v2-l1-x2-fp16.onnx",
];

if (!fs.existsSync(SRC)) {
  console.error(`组件目录不存在：${SRC}\n先按文档组装运行时（取自 mpv-Yaozhi rife-ncnn-vulkan）再打包。`);
  process.exit(1);
}
for (const rel of REQUIRED) {
  if (!fs.existsSync(path.join(SRC, rel))) {
    console.error(`缺关键文件：${rel}`);
    process.exit(1);
  }
}
const local = JSON.parse(fs.readFileSync(path.join(SRC, "manifest.json"), "utf8"));
const version = process.argv[2] || local.version || "1.0.0";

fs.mkdirSync(OUT_DIR, { recursive: true });
const zip = path.join(OUT_DIR, `anibox-ai-enhance-${version}.zip`);
if (fs.existsSync(zip)) fs.rmSync(zip);

// pwsh Compress-Archive：排除 manifest.json（远端 manifest 单独生成，含 zip 自身的 sha）
const list = ["vs", "runtime", "licenses"].filter((d) => fs.existsSync(path.join(SRC, d)));
execFileSync("pwsh", [
  "-NoProfile", "-Command",
  `Compress-Archive -Path ${list.map((d) => `'${path.join(SRC, d)}'`).join(",")} -DestinationPath '${zip}' -Force`,
]);
const buf = fs.readFileSync(zip);
const sha = crypto.createHash("sha256").update(buf).digest("hex");
const manifest = {
  version,
  zipUrl: `https://ai.690075.xyz/anibox-ai-enhance-${version}.zip`,
  zipSha256: sha,
  sizeBytes: buf.length,
  minAppVersion: "4.1.0",
  notes: "RIFE v4.6 ncnn Vulkan + VapourSynth R79 runtime (all-GPU), from mpv-Yaozhi 1.0.6-2 rife-ncnn-vulkan",
};
fs.writeFileSync(path.join(OUT_DIR, "manifest.json"), JSON.stringify(manifest, null, 2));
console.log(`zip:   ${zip}`);
console.log(`size:  ${(buf.length / 1048576).toFixed(1)} MB`);
console.log(`sha256: ${sha}`);
console.log(`manifest: ${path.join(OUT_DIR, "manifest.json")}`);
console.log("上传：manifest.json 与 zip 均放 R2 桶根；enhance.rs 的 AI_ENHANCE_MANIFEST_URL 指向该 manifest。");
