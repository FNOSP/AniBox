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
  "vs/mvtools.vpy",
  "vs/superres.vpy",
  "vs/superres_trt.vpy",
  "runtime/Lib/site-packages/vapoursynth/vsscript.dll",
  "runtime/libvapoursynth.dll",
  "runtime/plugins/librife.dll",
  "runtime/plugins/MiscFilters.dll",
  // MVTools 运动补偿补帧插件（官方 VapourSynth mvtools v24 win64 预编译版）。
  // 来源：https://github.com/dubhater/vapoursynth-mvtools/releases/tag/v24
  //   vapoursynth-mvtools-v24-win64.7z → libmvtools.dll（重命名为 mvtools.dll）
  //   12,674,089 字节（≈12.1MB），x64 PE（machine=0x8664），导出 VapourSynthPluginInit
  //   （API3，R79 兼容），SHA-256 530a2f003901a7c0…（与 vsrepo 官方清单一致）。
  // 注意：v25+ 官方 release 不含 Windows 二进制，故 Windows x64 以 v24 为准。
  "runtime/plugins/mvtools.dll",
  // [lc-2012] open-svpflow（SVP 开源重写版 Z1xus/open-svpflow，OpenCL GPU 加速）——
  // 实测 5070 HQ 档 720p 6.68× / 1080p 5.76× 实时，性能最好的真实补帧引擎。
  "runtime/plugins/svpflow1_vs.dll",
  "runtime/plugins/svpflow2_vs.dll",
  "vs/open-svpflow.vpy",
  // vsort + onnxruntime + DirectML 仅供 AI 超分 DML 档（AnimeJaNai）；
  // [lc-2004] RIFE DirectML 后端已删（实测 ONNX 会话初始化异常），rife_dml.vpy 与
  // runtime/plugins/models/rife_v2/ 不再入包。
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

// [lc-2004] packs（TRT 大包等扩展包）不随基础包重打——从线上 manifest 原样保留，
// 防止重发基础包时把 packs.tensorrt 抹掉（TRT 下载入口会报「清单缺 packs.tensorrt」）。
let packs;
try {
  const res = await fetch("https://ai.690075.xyz/manifest.json");
  const remote = await res.json();
  if (remote.packs && typeof remote.packs === "object") packs = remote.packs;
} catch {
  console.warn("（拉不到线上 manifest，packs 段将缺失——如需 TRT 大包入口请手工合并）");
}

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
if (packs) manifest.packs = packs;
fs.writeFileSync(path.join(OUT_DIR, "manifest.json"), JSON.stringify(manifest, null, 2));
console.log(`zip:   ${zip}`);
console.log(`size:  ${(buf.length / 1048576).toFixed(1)} MB`);
console.log(`sha256: ${sha}`);
console.log(`manifest: ${path.join(OUT_DIR, "manifest.json")}`);
console.log("上传：manifest.json 与 zip 均放 R2 桶根；enhance.rs 的 AI_ENHANCE_MANIFEST_URL 指向该 manifest。");
