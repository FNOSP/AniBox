#!/usr/bin/env node
/**
 * [lc-1350] Fntv-Plus 主进程热补丁器 —— PotPlayer 播放僵尸流修复（一键 cmd 的内嵌载荷）。
 *
 * 原理：利用应用自带的 patchOverlay 文件级覆盖钩子（main.ts 第一行安装）——
 * 任何 require 解析到 <安装目录>/resources/app.asar 内 dest/... 文件时，若
 * <userData>/patches/<同相对路径> 存在同名文件，则优先加载覆盖文件。
 * 本工具从用户自己的 app.asar 抽出原 dest/main/common/playbackShim.js，
 * 施加两处外科手术式替换后写入 userData/patches/main/common/playbackShim.js：
 *   ① res 'close' 的回收条件去掉 !responded 门：responded 在上游响应头到达时（毫秒级）
 *      即置真，播放器「读几百 KB 就弃连」的探测/拖动请求全部落在其后无人回收——
 *      pipe 路径里 pipe 之前挂的计数 data 监听器让上游保持 flowing，数据被全速读入
 *      后直接丢弃、零背压，一条被弃的 `bytes=X-` 请求就会持续拉完整个文件尾
 *      （86GB 实测把 NAS 千兆打满、关掉播放器也不停）。改为 !res.writableEnded：
 *      没发完就掐，正常发完（writableEnded=true）绝不误杀。
 *   ② 上游响应回调入口注入 pres 'error' 守卫：①会主动 destroy 上游，pres 必收到
 *      ECONNRESET；原代码 pipe/嗅探路径均无 error 监听，不接住就是主进程未捕获异常。
 *
 * 版本兼容策略：不整文件替换（各 3.8.0 构建子版本之间该文件存在漂移，整换会引入
 * 依赖不一致），只做锚点替换；锚点为 lc-1004 以来一直稳定的代码段，出现次数不符
 * 即中止，绝不半途写坏。restore 模式删除覆盖文件即回到安装包原样。
 */
'use strict';
const fs = require('fs');
const path = require('path');

const TARGET = 'dest/main/common/playbackShim.js';
const OVERLAY_REL = 'main/common/playbackShim.js';
const META_SUFFIX = '.lc1350.meta.json';
// 版本号标记：写入 config.json 的 appliedPatchVersion，应用内「当前版本」即显示它
// （update.ts: getAppliedPatchVersion() 优先级最高）。格式必须可被 versionGreater 的
// stripVersionSuffix 剥掉（-(?:hotfix|full|test)\d*$），否则未来官方热补丁的版本比较会失准。
const HOTFIX_VERSION = '3.8.0-hotfix';

// ---- 锚点与替换（对齐 lc-1004 以来编译产物中稳定的代码段）----
const R_OLD_CLOSE = 'if (!responded && !res.writableEnded) {';
const R_NEW_CLOSE = 'if (!res.writableEnded) {';
const R_CB_OPEN = 'requester.request(options, (pres) => {';
const R_GUARD = ' pres.on("error", function () { try { res.destroy(); } catch (e) {} });';

function countOf(hay, needle) {
    let n = 0, i = 0;
    while ((i = hay.indexOf(needle, i)) !== -1) { n++; i += needle.length; }
    return n;
}
function argVal(name) {
    const i = process.argv.indexOf('--' + name);
    return i >= 0 ? process.argv[i + 1] : undefined;
}
const hasFlag = (n) => process.argv.includes('--' + n);
function fail(code, msg) {
    console.error('[lc-1350] ✗ ' + msg);
    process.exit(code);
}
function sha256(buf) { return require('crypto').createHash('sha256').update(buf).digest('hex'); }

/** 完整读取（Windows 常规文件单次 readSync 即可，这里循环兜底） */
function readFull(fd, buf, fileOffset) {
    let done = 0;
    while (done < buf.length) {
        const n = fs.readSync(fd, buf, done, buf.length - done, fileOffset + done);
        if (n <= 0) throw new Error('读取中断 at ' + (fileOffset + done));
        done += n;
    }
}

/**
 * 最小 asar 读取器。@electron/asar 磁盘格式（双层 pickle）：
 *   [0..4)   uint32 = 4（外层 size pickle 的 payload 长度）
 *   [4..8)   uint32 = 头 pickle 总长 L
 *   头 pickle（文件偏移 8 起，长 L）：
 *     [8..12)  uint32 = 头 pickle payload 长度 P
 *     [12..16) uint32 = JSON 字节长 N
 *     [16..16+N)       头 JSON（files 树，offset 为相对数据区起点的十进制字符串）
 *   数据区起点 = 8 + L
 */
function readAsarFile(asarPath, relPath) {
    const fd = fs.openSync(asarPath, 'r');
    try {
        const head = Buffer.alloc(20);
        readFull(fd, head, 0);
        if (head.readUInt32LE(0) !== 4) throw new Error('魔数不符，不是有效的 asar 文件');
        const headerSize = head.readUInt32LE(4);
        const jsonLen = head.readUInt32LE(12);
        if (jsonLen <= 0 || jsonLen > 64 * 1024 * 1024) throw new Error('asar 头长度异常: ' + jsonLen);
        const json = Buffer.alloc(jsonLen);
        readFull(fd, json, 16);
        const root = JSON.parse(json.toString('utf8'));
        const dataStart = 8 + headerSize;
        let node = root;
        for (const seg of relPath.split('/')) {
            node = node && node.files && node.files[seg];
            if (!node) return null;
        }
        if (!node || node.files || node.unpacked || node.offset === undefined) return null;
        const size = Number(node.size);
        const off = dataStart + Number(node.offset);
        const buf = Buffer.alloc(size);
        readFull(fd, buf, off);
        return buf;
    } finally {
        fs.closeSync(fd);
    }
}

/**
 * 读写 config.json 的 appliedPatchVersion / appliedPatchSignature（明文字段）。
 *
 * ⚠️ 两字段必须成对写：启动对账(reconcilePatchStateOnStartup)发现 appliedPatchVersion
 * 有值而签名为空时，会按「修复前旧版应用的补丁」清空整个覆盖层。签名算法与
 * patchApplier.computeInstallSignature 完全一致 = 主 exe 的 mtimeMs 字符串。
 * remove=true 时只清除【本补丁】的标记（版本号不是 3.8.0-hotfix 则视为其它补丁，保留不动）。
 */
function writeVersionMarker(patchesDir, exePath, remove) {
    const cfgPath = path.join(path.dirname(patchesDir), 'config.json');
    let cfg = {};
    try {
        if (fs.existsSync(cfgPath)) cfg = JSON.parse(fs.readFileSync(cfgPath, 'utf8')) || {};
    } catch (e) {
        return { ok: false, msg: 'config.json 解析失败（' + e.message + '），版本号标记跳过' };
    }
    if (remove) {
        if (cfg.appliedPatchVersion && cfg.appliedPatchVersion !== HOTFIX_VERSION) {
            return { ok: true, msg: '检测到其它补丁版本(' + cfg.appliedPatchVersion + ')，其版本号标记保留不动' };
        }
        delete cfg.appliedPatchVersion;
        delete cfg.appliedPatchSignature;
    } else {
        let sig = '';
        try {
            // 门禁：签名必须来自真实主 exe（basename = Fntv-Plus.exe），错签名会让启动对账
            // 误判「安装包已变更」而清空整个覆盖层。FNTV_ALLOW_ANY_EXE=1 仅供自测旁路。
            const anyExe = process.env.FNTV_ALLOW_ANY_EXE === '1';
            if (exePath && fs.existsSync(exePath)
                && (anyExe || path.basename(exePath).toLowerCase() === 'fntv-plus.exe')) {
                sig = String(fs.statSync(exePath).mtimeMs);
            }
        } catch { /* 签名取不到则不写标记 */ }
        if (!sig) {
            return { ok: false, msg: '未取到 Fntv-Plus.exe 安装签名，版本号标记跳过（修复本身不受影响）' };
        }
        cfg.appliedPatchVersion = HOTFIX_VERSION;
        cfg.appliedPatchSignature = sig;
    }
    try {
        fs.writeFileSync(cfgPath, JSON.stringify(cfg, null, 2));
        return { ok: true, path: cfgPath };
    } catch (e) {
        return { ok: false, msg: 'config.json 写入失败（' + e.message + '）——应用正在运行时退出会回写配置，请退出应用后重跑本补丁' };
    }
}

/**
 * 读取 asar 内目标文件的原始字节 —— 按运行时分策略：
 *  - 应用自带 Electron（含 ELECTRON_RUN_AS_NODE）：Electron 对 *.asar 路径有全局 fs 拦截，
 *    裸 archive 路径会被当「空子路径」拒读（ENOENT, '' not found in ...），连 \\?\ 前缀也拦；
 *    但拦截 fs 的设计用途就是读包内文件 —— 走 <archive>/<相对路径> 直读即可。
 *  - 纯 Node（自测/CI）：fs 无拦截，用上面的最小 asar 读取器按头 JSON+offset 自行抽取。
 */
function readOriginalFile(asarPath, relPath) {
    if (process.versions && process.versions.electron) {
        return fs.readFileSync(path.join(asarPath, relPath));
    }
    return readAsarFile(asarPath, relPath);
}

function pickPatchesDir() {
    const primary = argVal('patches');
    const alt = argVal('patches-alt');
    if (primary && !alt) return primary; // 单一显式目录（测试直用）
    if (primary && alt) {
        // cmd 会同时给两个候选（不同历史版本的 userData 根不同）：存在的优先，都不存在则建 primary
        if (fs.existsSync(primary)) return primary;
        if (fs.existsSync(alt)) return alt;
        return primary;
    }
    const cands = [alt, process.env.APPDATA ? path.join(process.env.APPDATA, 'fntv', 'patches') : '',
        process.env.APPDATA ? path.join(process.env.APPDATA, 'Fntv-Plus', 'patches') : ''].filter(Boolean);
    // userData 根已存在的优先（logs 目录是「这个 userData 真在用」的强证据）
    let best = null, bestM = -1;
    for (const c of cands) {
        const root = path.dirname(c);
        if (!fs.existsSync(root)) continue;
        let m = 0;
        try { m = fs.statSync(path.join(root, 'logs')).mtimeMs; } catch { /* 无 logs 记 0 */ }
        if (best === null || m > bestM) { best = c; bestM = m; }
    }
    return best || cands[0];
}

function main() {
    console.log('[lc-1350] Fntv-Plus 热补丁器（PotPlayer 播放僵尸流修复）');
    const restore = hasFlag('restore');
    const patchesDir = pickPatchesDir();
    if (!patchesDir) fail(3, '无法确定补丁目录（%APPDATA% 缺失？）');
    const overlayPath = path.join(patchesDir, OVERLAY_REL);
    const metaPath = overlayPath + META_SUFFIX;

    if (restore) {
        const cfgPath = path.join(path.dirname(patchesDir), 'config.json');
        let curVer = '';
        try {
            if (fs.existsSync(cfgPath)) curVer = (JSON.parse(fs.readFileSync(cfgPath, 'utf8')) || {}).appliedPatchVersion || '';
        } catch { /* 读取失败按无标记处理 */ }
        if (curVer && curVer !== HOTFIX_VERSION) {
            console.log('[lc-1350] ⚠ 检测到当前应用的是其它补丁(v' + curVer + ')，本命令不做任何改动；');
            console.log('[lc-1350]   如需回滚请使用应用内「回滚补丁」按钮。');
            return;
        }
        let removed = 0;
        for (const f of [overlayPath, metaPath]) {
            try { if (fs.existsSync(f)) { fs.unlinkSync(f); removed++; } } catch (e) { fail(3, '删除失败 ' + f + ': ' + e.message); }
        }
        const vm = writeVersionMarker(patchesDir, '', true);
        console.log(removed ? `[lc-1350] ✓ 已卸载补丁（删除 ${removed} 个覆盖文件），重启应用即回到安装包原样`
            : '[lc-1350] 本来就没有安装过本补丁，无需卸载');
        if (vm.ok) console.log('[lc-1350] ✓ 版本号标记已清除，应用内「当前版本」恢复为安装包版本');
        else console.log('[lc-1350] ⚠ ' + vm.msg);
        console.log('[lc-1350] 覆盖文件位置: ' + overlayPath);
        return;
    }

    const asarPath = argVal('asar');
    if (!asarPath || !fs.existsSync(asarPath)) fail(3, '未找到 app.asar: ' + asarPath);
    console.log('[lc-1350] 安装包: ' + asarPath);
    console.log('[lc-1350] 补丁目录: ' + patchesDir);

    let origBuf;
    try {
        origBuf = readOriginalFile(asarPath, TARGET);
    } catch (e) {
        fail(2, '读取 asar 失败: ' + e.message);
    }
    if (!origBuf) fail(2, `安装包内未找到 ${TARGET}，应用版本不兼容（本补丁面向 3.6+）`);

    const original = origBuf.toString('utf8');
    // 往返一致性自检：字符串操作后重编码必须与原字节一致（未替换区域零改动）
    if (!Buffer.from(original, 'utf8').equals(origBuf)) fail(2, '源文件编码自检失败，已中止');

    const cOld = countOf(original, R_OLD_CLOSE);
    const already = cOld === 0 && original.includes(R_NEW_CLOSE);
    let patchedStr;
    if (already) {
        patchedStr = original;
        console.log('[lc-1350] 检测到安装包已是修复版（此前打过本补丁或更新版已含修复），直接写入覆盖层');
    } else {
        if (cOld !== 1) fail(2, `锚点①（close 回收条件）出现 ${cOld} 次（预期 1），此版本不兼容，已中止（未做任何改动）`);
        const c2 = countOf(original, R_CB_OPEN);
        if (c2 !== 1) fail(2, `锚点②（上游响应回调入口）出现 ${c2} 次（预期 1），此版本不兼容，已中止（未做任何改动）`);
        patchedStr = original.replace(R_OLD_CLOSE, R_NEW_CLOSE).replace(R_CB_OPEN, R_CB_OPEN + R_GUARD);
        if (countOf(patchedStr, R_GUARD) !== 1 || countOf(patchedStr, R_NEW_CLOSE) !== 1) {
            fail(2, '替换后自检失败，已中止（未写入任何文件）');
        }
        console.log('[lc-1350] ✓ 已应用 2 处外科替换（close 回收无条件化 + pres error 守卫注入）');
    }

    fs.mkdirSync(path.dirname(overlayPath), { recursive: true });
    const patchedBuf = Buffer.from(patchedStr, 'utf8');
    fs.writeFileSync(overlayPath, patchedBuf);
    fs.writeFileSync(metaPath, JSON.stringify({
        hotfix: 'lc-1350',
        appliedAt: new Date().toISOString(),
        source: { sha256: sha256(origBuf), bytes: origBuf.length },
        patched: { sha256: sha256(patchedBuf), bytes: patchedBuf.length },
        overlays: 'main/common/playbackShim.js (patchOverlay 文件级覆盖)',
    }, null, 2));

    // 回读校验
    const back = fs.readFileSync(overlayPath);
    if (!back.equals(patchedBuf)) fail(3, '写入后回读不一致，请检查磁盘/杀软');
    console.log('[lc-1350] ✓ 覆盖文件已写入并通过回读校验: ' + overlayPath);

    // 版本号标记：应用内「当前版本」显示 3.8.0-hotfix（getAppliedPatchVersion 优先级最高）
    const vm = writeVersionMarker(patchesDir, argVal('exe'), false);
    if (vm.ok) console.log('[lc-1350] ✓ 版本号已标记为 ' + HOTFIX_VERSION + '，重启应用后「当前版本」即显示该值');
    else console.log('[lc-1350] ⚠ ' + vm.msg);

    try {
        const { execSync } = require('child_process');
        const out = execSync('tasklist /FI "IMAGENAME eq Fntv-Plus.exe" /NH 2>nul', { encoding: 'utf8' });
        if (/Fntv-Plus\.exe/i.test(out)) {
            console.log('[lc-1350] ⚠ 检测到应用正在运行：请完全退出飞牛影视后重新打开，补丁才会生效');
        } else {
            console.log('[lc-1350] 下次启动飞牛影视即自动生效（当前应用未在运行，无需额外操作）');
        }
    } catch { /* 检测失败不影响补丁 */ }
    console.log('[lc-1350] 效果：PotPlayer 弃连的探测/拖动请求即时中止，NAS 千兆不再被僵尸流占满，');
    console.log('[lc-1350]       播放带宽恢复正常，关闭播放器后 NAS 上传立即停止。');
    console.log('[lc-1350] 卸载方法：以 restore 参数重新运行本补丁（或应用内「回滚补丁」）。');
}

if (require.main === module) {
    try {
        main();
    } catch (e) {
        console.error('[lc-1350] ✗ 未预期异常: ' + (e && e.stack || e));
        process.exit(3);
    }
}
module.exports = { readAsarFile };
