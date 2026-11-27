#!/usr/bin/env node
/**
 * tools/stage-bundle-resources.mjs
 *
 * 把随包运行时资源从本机 `third_party/` 裁剪复制到 `src-tauri/bundle-resources/`，
 * 供 Tauri `bundle.resources` 打包（见 src-tauri/tauri.conf.json）。
 *
 * 为什么不直接在 tauri.conf.json 里引用 `../third_party/fntv-mpv/`：
 *   Tauri v2 的 resources 不支持 glob 排除（`!pattern`），而 `third_party/fntv-mpv`
 *   里混有开发机的用户状态与凭据：
 *     portable_config/files/{recent.json,persistent_config.json}
 *     portable_config/scripts/uosc_danmaku/bili_cookie.txt
 *     portable_config/scripts/uosc_danmaku/bili_cookie_status.json
 *     portable_config/scripts/uosc_danmaku/danmaku_source_cache.json
 *     portable_config/{cache,watch_later,scripts-backup}、*.bak*、mpv.log
 *   直接整目录打包 = 把 B 站 Cookie 和播放记录发给所有用户。因此先在这里白名单
 *   复制，再由 Tauri 打包干净的 staging 目录。
 *
 * 该脚本由 tauri.conf.json 的 beforeBuildCommand 调用；开发态由 beforeDevCommand
 * 以 `--dev` 调用（只保证空目录存在，不复制 280MB，运行时仍读仓库 third_party/，
 * 见 config.rs::resolve_bundled_paths）。
 *
 * 增量：staging 目录里写 `.stamp`（源文件清单 + size + mtime 的 sha256），
 * 源没变就直接跳过，避免每次 build 重复复制 ~280MB。
 */
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const IS_WIN = process.platform === 'win32';
const SRC_MPV = path.join(ROOT, 'third_party', 'fntv-mpv');
// Windows 是 proxy.exe；Linux/macOS 由 Go 源码构建出无后缀的 proxy
const PROXY_NAME = IS_WIN ? 'proxy.exe' : 'proxy';
const SRC_PROXY = path.join(ROOT, 'third_party', 'proxy', PROXY_NAME);
const OUT = path.join(ROOT, 'src-tauri', 'bundle-resources');
const STAMP = path.join(OUT, '.stamp');

const DEV = process.argv.includes('--dev');
const OPTIONAL = process.argv.includes('--optional');

const PC = path.join(SRC_MPV, 'portable_config');

/** portable_config 根目录白名单（其余根文件一律不要）。 */
const PC_ROOT_FILES = new Set([
  'mpv.conf',
  'mpv-user.conf',
  'mpv-interp-nvidia.conf',
  'input.conf',
]);

/** portable_config 子目录白名单。 */
const PC_DIRS = new Set([
  'fonts',
  'shaders',
  'scripts',
  'script-modules',
  'script-assets',
  'script-opts',
]);

/** 任意层级都排除的目录名。 */
const SKIP_DIRS = new Set([
  'cache',
  'watch_later',
  'files',
  'scripts-backup',
  '__pycache__',
]);

/**
 * 文件名/后缀排除：用户状态、日志、备份、凭据。
 * `anibox-shaders.conf` 是 fntv_shader.lua 的用户着色器持久化文件（首选在 %APPDATA%，
 * 便携兜底在 portable_config 根目录）。它绝不能进安装包，否则升级会覆盖用户选择。
 */
const SKIP_FILE_RE =
  /(\.bak($|[.\-_])|\.log$|^anibox-shaders\.conf$|^bili_cookie\.txt$|^bili_cookie_status\.json$|^danmaku_source_cache\.json$|^danmaku-history\.json$|^danmaku_blacklist\.txt$|^recent\.json$|^persistent_config\.json$)/i;

function fail(msg) {
  console.error(`[stage-resources] ${msg}`);
  process.exit(1);
}

/** [lc-2196] 必须随安装包分发的许可文件（相对仓库根）。 */
const LICENSE_FILES = ['LICENSE', 'THIRD_PARTY_NOTICES.md'];

/** [lc-2196] 随包第三方许可副本目录（仓库内权威副本，不依赖本地 third_party/）。 */
const LICENSES_DIR = 'licenses';

/** Tauri 的 resources 目录模式在空目录上是合法的；先建好骨架，dev / 新 clone 也能编译。 */
function ensureDirs() {
  fs.mkdirSync(path.join(OUT, 'third_party', 'fntv-mpv'), { recursive: true });
  fs.mkdirSync(path.join(OUT, 'third_party', 'proxy'), { recursive: true });
  /* [lc-2163] duanju 也要建骨架：tauri.conf.json 的 bundle.resources 里**无条件**声明了
     `bundle-resources/third_party/duanju/`，但短剧核心（guoapp duanju_core.dll）只有 Windows
     amd64 有，Linux/macOS 上游未提供。此前这里漏建目录 → CI 上 Tauri 直接报
     `resource path 'bundle-resources/third_party/duanju' doesn't exist` 并让三个平台全部构建失败。
     空目录是合法的（Tauri 允许空资源目录），Windows 侧有文件时照常拷进去。 */
  fs.mkdirSync(path.join(OUT, 'third_party', 'duanju'), { recursive: true });
  /* [lc-2196] 许可文件也要 staging：tauri.conf.json 的 bundle.resources 声明了
     `bundle-resources/LICENSE` 与 `bundle-resources/THIRD_PARTY_NOTICES.md`。
     AGPL-3.0 §4/§6 要求分发目标码时随附许可证副本；此前安装目录里没有任何
     声明文件，用户装上后在 `<安装目录>/` 找不到归属与许可信息。 */
  for (const f of LICENSE_FILES) {
    fs.copyFileSync(path.join(ROOT, f), path.join(OUT, f));
  }
  /* 第三方许可副本整目录复制（fonts/ 下的 OFL / Apache-2.0 等）。 */
  const licSrc = path.join(ROOT, LICENSES_DIR);
  if (fs.existsSync(licSrc)) {
    fs.rmSync(path.join(OUT, LICENSES_DIR), { recursive: true, force: true });
    fs.cpSync(licSrc, path.join(OUT, LICENSES_DIR), { recursive: true });
  }
}

function walk(dir, relBase, out) {
  for (const ent of fs.readdirSync(dir, { withFileTypes: true })) {
    const abs = path.join(dir, ent.name);
    const rel = relBase ? `${relBase}/${ent.name}` : ent.name;
    if (ent.isDirectory()) {
      if (SKIP_DIRS.has(ent.name)) continue;
      walk(abs, rel, out);
    } else if (ent.isFile()) {
      if (SKIP_FILE_RE.test(ent.name)) continue;
      out.push({ abs, rel });
    }
  }
}

/** 计算本次应复制的源文件清单（同时作为 stamp 输入）。 */
function collect() {
  if (!fs.existsSync(SRC_MPV) || !fs.existsSync(SRC_PROXY)) {
    if (OPTIONAL) {
      console.warn(
        '[stage-resources] third_party/ 不完整，已跳过 staging（开发态可运行；发布构建需补齐）',
      );
      ensureDirs();
      process.exit(0);
    }
    if (!fs.existsSync(SRC_MPV)) {
      fail(
        `缺少 ${SRC_MPV}\n` +
          '发布构建需要本地准备 third_party/fntv-mpv（Windows：mpv.exe + libmpv + portable_config；' +
          'Linux/macOS：至少需要 portable_config，供系统 mpv 使用）。\n' +
          '该目录按仓库规则不入库；开发态可直接运行 dev.cmd，不需要 staging。',
      );
    }
    fail(
      `缺少 ${SRC_PROXY}\n` +
        `发布构建需要本地准备 third_party/proxy/${PROXY_NAME}（Go 代理；` +
        (IS_WIN ? 'Windows 用预编译的 proxy.exe' : 'Linux/macOS 由 proxy/ 源码 go build 生成') +
        '）。',
    );
  }

  const items = [];

  // 1) Windows：mpv 根目录的运行时二进制（DLL/exe/com）。Linux/macOS 用系统 mpv，
  //    只打包 portable_config，绝不把 Windows 二进制塞进非 Windows 安装包。
  if (IS_WIN) {
    for (const ent of fs.readdirSync(SRC_MPV, { withFileTypes: true })) {
      if (!ent.isFile()) continue;
      if (!/\.(dll|exe|com)$/i.test(ent.name)) continue;
      items.push({
        abs: path.join(SRC_MPV, ent.name),
        rel: `third_party/fntv-mpv/${ent.name}`,
      });
    }
    // mpv/fonts.conf（mpv 的 fontconfig 配置）
    const fontsConf = path.join(SRC_MPV, 'mpv', 'fonts.conf');
    if (fs.existsSync(fontsConf)) {
      items.push({ abs: fontsConf, rel: 'third_party/fntv-mpv/mpv/fonts.conf' });
    }
  }

  // 2) 上游 mpv 脚本的构建期补丁（third_party 不入库，CI 每次拉新包覆盖，
  //    所以修复必须在这里重新打一次，不能只改本地文件）。
  //    [lc-2334] patch-mpv-icon.mjs 也在这组里：它改写 mpv.exe 自身资源段里的
  //    图标位图（运行时跨进程 WM_SETICON 对 Win11 任务栏无效），所以必须在
  //    下面复制到 bundle-resources 之前完成。
  const patchTools = [
    'patch-fntv-mpv.mjs',
    'patch-uosc-gate.mjs',
    'patch-modernz-contrast.mjs',
    // [lc-2341/2338] 处理顺序流（modernz 详情弹窗一行）+ 插帧 ResumePrime
    'patch-anibox-mpv.mjs',
  ];
  if (IS_WIN) patchTools.push('patch-mpv-icon.mjs');
  for (const tool of patchTools) {
    const p = path.join(ROOT, 'tools', tool);
    if (!fs.existsSync(p)) continue;
    try {
      execFileSync(process.execPath, [p], { stdio: 'inherit' });
    } catch (e) {
      fail(`${tool} 打补丁失败：${e.message}`);
    }
  }

  // 3) 自有 mpv 脚本（tools/mpv-scripts → portable_config/scripts）：
  //    先同步再收集，保证本地/CI 拿到的都是仓库里那一份（third_party 不入库，
  //    R2 配置包里没有这些文件）。
  try {
    execFileSync(process.execPath, [path.join(ROOT, 'tools', 'install-mpv-scripts.mjs')], {
      stdio: 'inherit',
    });
  } catch {
    /* 源目录不存在等异常不该拦住打包：install 脚本自己会安静跳过 */
  }

  // 4) portable_config：白名单根文件 + 白名单子目录。
  if (!fs.existsSync(PC)) {
    fail(`缺少 ${PC}（portable_config 是各平台都需要的便携配置）`);
  }
  for (const ent of fs.readdirSync(PC, { withFileTypes: true })) {
    const abs = path.join(PC, ent.name);
    const rel = `third_party/fntv-mpv/portable_config/${ent.name}`;
    if (ent.isFile()) {
      if (!PC_ROOT_FILES.has(ent.name)) continue;
      if (SKIP_FILE_RE.test(ent.name)) continue;
      items.push({ abs, rel });
    } else if (ent.isDirectory() && PC_DIRS.has(ent.name)) {
      const nested = [];
      walk(abs, rel, nested);
      items.push(...nested);
    }
  }

  // 2.5) [lc-2078] 短剧核心（guoapp duanju_core.dll；Windows amd64 随包，
  //      Linux/macOS 上游核心待供给——见 tools/fetch-duanju-core.py）。
  //      [lc-2180] v2 架构站源 = JS 模块（third_party/duanju/modules/<id>/…），
  //      随核心一并随包，运行时按版本部署到数据目录 modules/。
  const duanjuDir = path.join(ROOT, 'third_party', 'duanju');
  if (fs.existsSync(duanjuDir)) {
    for (const ent of fs.readdirSync(duanjuDir, { withFileTypes: true })) {
      if (ent.isFile() && /\.(dll|so|dylib)$/i.test(ent.name)) {
        items.push({ abs: path.join(duanjuDir, ent.name), rel: `third_party/duanju/${ent.name}` });
      }
    }
    const duanjuModules = path.join(duanjuDir, 'modules');
    if (fs.existsSync(duanjuModules)) {
      const nested = [];
      walk(duanjuModules, 'third_party/duanju/modules', nested);
      items.push(...nested);
    }
  }

  // 3) Go 代理（potctl.exe 仅 Windows，已在 src-tauri/resources 里，单独映射）。
  items.push({ abs: SRC_PROXY, rel: `third_party/proxy/${PROXY_NAME}` });

  items.sort((a, b) => a.rel.localeCompare(b.rel));
  return items;
}

function signature(items) {
  const h = crypto.createHash('sha256');
  for (const it of items) {
    const st = fs.statSync(it.abs);
    h.update(`${it.rel}\0${st.size}\0${st.mtimeMs}\n`);
  }
  /* [lc-2196] 许可文件不在 items 里（由 ensureDirs 固定复制），但同样要纳入签名：
     否则改了 LICENSE / THIRD_PARTY_NOTICES.md 后 stamp 不变，staging 直接跳过，
     安装包里会带着过期的许可证副本。 */
  for (const f of LICENSE_FILES) {
    const st = fs.statSync(path.join(ROOT, f));
    h.update(`${f}\0${st.size}\0${st.mtimeMs}\n`);
  }
  return h.digest('hex');
}

function main() {
  // 开发态：只保证目录骨架存在，避免复制 280MB；运行时直接读仓库 third_party/。
  if (DEV) {
    ensureDirs();
    console.log(`[stage-resources] dev mode: ensured ${OUT}`);
    return;
  }
  const items = collect();
  const sig = signature(items);

  if (fs.existsSync(STAMP) && fs.readFileSync(STAMP, 'utf8').trim() === sig) {
    const mb = items.reduce((n, it) => n + fs.statSync(it.abs).size, 0) / 1048576;
    console.log(`[stage-resources] up-to-date (${items.length} files, ${mb.toFixed(1)} MB) -> ${OUT}`);
    return;
  }

  fs.rmSync(OUT, { recursive: true, force: true });
  /* [lc-2163] 清空后重建骨架目录（含 third_party/duanju）。
     只靠逐文件 mkdirSync(dirname) 是建不出**空目录**的：
     Linux/macOS 上短剧核心不存在 → 没有任何文件落在 duanju/ 下 → 目录缺失
     → Tauri 报 `resource path 'bundle-resources/third_party/duanju' doesn't exist`
     → 三个平台的 CI 全部构建失败。 */
  ensureDirs();
  let bytes = 0;
  for (const it of items) {
    const dst = path.join(OUT, it.rel);
    fs.mkdirSync(path.dirname(dst), { recursive: true });
    fs.copyFileSync(it.abs, dst);
    bytes += fs.statSync(it.abs).size;
  }
  fs.writeFileSync(STAMP, `${sig}\n`, 'utf8');
  console.log(
    `[stage-resources] staged ${items.length} files, ${(bytes / 1048576).toFixed(1)} MB -> ${OUT}`,
  );
}

main();
