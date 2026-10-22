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
  /(\.bak($|[.\-_])|\.log$|^anibox-shaders\.conf$|^bili_cookie\.txt$|^bili_cookie_status\.json$|^danmaku_source_cache\.json$|^danmaku-history\.json$|^recent\.json$|^persistent_config\.json$)/i;

function fail(msg) {
  console.error(`[stage-resources] ${msg}`);
  process.exit(1);
}

/** Tauri 的 resources 目录模式在空目录上是合法的；先建好骨架，dev / 新 clone 也能编译。 */
function ensureDirs() {
  fs.mkdirSync(path.join(OUT, 'third_party', 'fntv-mpv'), { recursive: true });
  fs.mkdirSync(path.join(OUT, 'third_party', 'proxy'), { recursive: true });
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

  // 2) portable_config：白名单根文件 + 白名单子目录。
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
