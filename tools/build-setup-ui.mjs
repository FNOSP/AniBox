#!/usr/bin/env node
// tools/build-setup-ui.mjs — 构建 AniBox 四页自绘安装器资源
//
// 产物（src-tauri/installer/staged/）：
//   AniBoxSetupUi.exe + WebView2 DLL + www.zip + app-meta.json + totalsize.txt + icon.ico
// 由 tauri.conf.json 的 build.beforeBundleCommand 在 Rust 二进制编译完成后调用，
// 因此 target/release/anibox.exe 已经存在，可以据此计算真实安装体积。
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, "..");
const INSTALLER = path.join(ROOT, "src-tauri", "installer");
const STAGED = path.join(INSTALLER, "staged");
const WWW = path.join(INSTALLER, "www");
const VENDOR = path.join(INSTALLER, "vendor");
const ICON = path.join(ROOT, "src-tauri", "icons", "icon.ico");
const MAIN_EXE = path.join(ROOT, "src-tauri", "target", "release", "anibox.exe");

function fail(msg) {
  console.error("[setup-ui] ERROR: " + msg);
  process.exit(1);
}

function log(msg) {
  console.log("[setup-ui] " + msg);
}

function readJson(p) {
  return JSON.parse(fs.readFileSync(p, "utf8"));
}

function walkSize(target) {
  let total = 0;
  const st = fs.statSync(target);
  if (st.isFile()) return st.size;
  for (const name of fs.readdirSync(target)) {
    total += walkSize(path.join(target, name));
  }
  return total;
}

function copyFile(from, to) {
  if (!fs.existsSync(from)) fail("缺少文件: " + from);
  fs.copyFileSync(from, to);
}

// ── 1. 读取版本 ──
const tauriConf = readJson(path.join(ROOT, "src-tauri", "tauri.conf.json"));
const pkg = readJson(path.join(ROOT, "package.json"));
const version = tauriConf.version || pkg.version;
if (!version) fail("无法从 tauri.conf.json / package.json 读取版本号");

// ── 2. 准备 staged 目录 ──
fs.rmSync(STAGED, { recursive: true, force: true });
fs.mkdirSync(STAGED, { recursive: true });

// ── 3. 编译 SetupUi.exe ──
const programFiles = process.env.ProgramFiles || "C:\\Program Files";
const programFilesX86 = process.env["ProgramFiles(x86)"] || "C:\\Program Files (x86)";
const cscCandidates = [
  process.env.CSC_EXE,
  path.join(programFiles, "Microsoft Visual Studio", "2022", "Community", "MSBuild", "Current", "Bin", "Roslyn", "csc.exe"),
  path.join(programFiles, "Microsoft Visual Studio", "2022", "Professional", "MSBuild", "Current", "Bin", "Roslyn", "csc.exe"),
  path.join(programFiles, "Microsoft Visual Studio", "2022", "Enterprise", "MSBuild", "Current", "Bin", "Roslyn", "csc.exe"),
  path.join(programFilesX86, "Microsoft Visual Studio", "2022", "BuildTools", "MSBuild", "Current", "Bin", "Roslyn", "csc.exe"),
  path.join(process.env.WINDIR || "C:\\Windows", "Microsoft.NET", "Framework64", "v4.0.30319", "csc.exe"),
].filter(Boolean);
const csc = cscCandidates.find((p) => fs.existsSync(p));
if (!csc) fail("找不到 csc.exe（需要 VS2022 Roslyn 或 .NET Framework 4.x）");

const net = path.join(process.env.WINDIR || "C:\\Windows", "Microsoft.NET", "Framework64", "v4.0.30319");
const gac = path.join(process.env.WINDIR || "C:\\Windows", "Microsoft.NET", "assembly");
const refs = [
  path.join(gac, "GAC_MSIL", "PresentationFramework", "v4.0_4.0.0.0__31bf3856ad364e35", "PresentationFramework.dll"),
  path.join(gac, "GAC_64", "PresentationCore", "v4.0_4.0.0.0__31bf3856ad364e35", "PresentationCore.dll"),
  path.join(gac, "GAC_MSIL", "WindowsBase", "v4.0_4.0.0.0__31bf3856ad364e35", "WindowsBase.dll"),
  path.join(gac, "GAC_MSIL", "System.Xaml", "v4.0_4.0.0.0__b77a5c561934e089", "System.Xaml.dll"),
  path.join(net, "System.Windows.Forms.dll"),
  path.join(net, "System.Drawing.dll"),
  path.join(net, "System.dll"),
  path.join(net, "System.Core.dll"),
  path.join(net, "System.IO.Compression.dll"),
  path.join(net, "System.IO.Compression.FileSystem.dll"),
  path.join(VENDOR, "Microsoft.Web.WebView2.Core.dll"),
  path.join(VENDOR, "Microsoft.Web.WebView2.Wpf.dll"),
];
for (const r of refs) if (!fs.existsSync(r)) fail("缺少编译引用: " + r);

const setupExe = path.join(STAGED, "AniBoxSetupUi.exe");
const cscArgs = [
  "-nologo",
  "-target:winexe",
  "-platform:anycpu",
  "-optimize+",
  "-codepage:65001",
  "-out:" + setupExe,
  "-win32manifest:" + path.join(INSTALLER, "app.manifest"),
  "-win32icon:" + ICON,
  ...refs.map((r) => "-r:" + r),
  path.join(INSTALLER, "InstallDetect.cs"),
  path.join(INSTALLER, "SetupUi.cs"),
];
log("编译 SetupUi.exe ...");
try {
  execFileSync(csc, cscArgs, { stdio: "inherit" });
} catch (e) {
  fail("csc 编译失败（退出码 " + e.status + "）");
}
if (!fs.existsSync(setupExe)) fail("csc 未生成 " + setupExe);
log("SetupUi.exe 编译完成: " + (fs.statSync(setupExe).size / 1024).toFixed(0) + " KB");

// ── 4. 复制 WebView2 运行时组件 / 图标 ──
for (const name of ["WebView2Loader.dll", "Microsoft.Web.WebView2.Core.dll", "Microsoft.Web.WebView2.Wpf.dll"]) {
  copyFile(path.join(VENDOR, name), path.join(STAGED, name));
}
copyFile(ICON, path.join(STAGED, "icon.ico"));

// ── 5. 打包 www.zip ──
for (const name of ["index.html", "app.css", "app.js", "license.html", "logo.png"]) {
  if (!fs.existsSync(path.join(WWW, name))) fail("www 资源缺失: " + name);
}
const zipPath = path.join(STAGED, "www.zip");
log("打包 www.zip ...");
const ps = `Compress-Archive -Path '${WWW.replace(/'/g, "''")}\\*' -DestinationPath '${zipPath.replace(/'/g, "''")}' -Force`;
try {
  execFileSync("pwsh", ["-NoProfile", "-Command", ps], { stdio: "inherit" });
} catch (e) {
  fail("Compress-Archive 失败（退出码 " + e.status + "）");
}
if (!fs.existsSync(zipPath)) fail("未生成 www.zip");
log("www.zip: " + (fs.statSync(zipPath).size / 1024).toFixed(0) + " KB");

// ── 6. app-meta.json ──
const now = new Date();
const pad = (n) => String(n).padStart(2, "0");
const releaseDate =
  `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())} ` +
  `${pad(now.getHours())}:${pad(now.getMinutes())}`;
const meta = { version, exeName: "anibox.exe", releaseDate };
fs.writeFileSync(path.join(STAGED, "app-meta.json"), JSON.stringify(meta), "utf8");
log("app-meta.json: " + JSON.stringify(meta));

// ── 7. totalsize.txt（安装后未压缩字节数，用于真实进度）──
if (!fs.existsSync(MAIN_EXE)) {
  fail("找不到主程序（请先完成 cargo/tauri 构建）: " + MAIN_EXE);
}
const resourceRoots = [
  path.join(ROOT, "src-tauri", "resources", "potctl.exe"),
  path.join(ROOT, "src-tauri", "resources", "danmu-api"),
  path.join(ROOT, "src-tauri", "bundle-resources", "third_party", "fntv-mpv"),
  path.join(ROOT, "src-tauri", "bundle-resources", "third_party", "proxy"),
];
let total = fs.statSync(MAIN_EXE).size;
for (const r of resourceRoots) {
  if (!fs.existsSync(r)) fail("缺少打包资源（请先运行 tools/stage-bundle-resources.mjs）: " + r);
  total += walkSize(r);
}
// uninstaller / 快捷方式 / 注册表开销的保守余量
total += 512 * 1024;
fs.writeFileSync(path.join(STAGED, "totalsize.txt"), String(total), "utf8");
log("totalsize.txt: " + total + " bytes (" + (total / 1024 / 1024).toFixed(1) + " MiB)");

// ── 8. 汇总 ──
const stagedFiles = fs.readdirSync(STAGED).sort();
log("staged 完成: " + stagedFiles.join(", "));
