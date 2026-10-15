#!/usr/bin/env node
/**
 * [lc-1560] 弹幕 shim：给 MPV 的 uosc_danmaku Lua 脚本提供 127.0.0.1:22347 的 HTTP 端点。
 *
 * 背景（迁移遗留）：主仓库用 Electron 主进程内的 playbackShim.ts 提供这组端点
 * （Lua 脚本硬编码调用 127.0.0.1:22347，见 main.lua / menu.lua / extra.lua）。
 * Tauri2 迁移时这部分没带过来 ⇒ B站弹幕下载失败 ⇒ 弹幕只剩弹弹play 兜底。
 *
 * 做法：不重写 B站逻辑，直接复用 portable_config 里现成的 bili_danmaku.js
 * （纯 Node 内置模块，可独立 require）。本文件只做「HTTP 路由 → 调 JS → 回 JSON」，
 * 与主仓库 playbackShim.ts 的端点契约保持一致。
 *
 * 端点（对齐主仓库 playbackShim.ts）：
 *   GET /danmaku?title=&ep=&out=&threshold=&season=&guid=&alt=
 *   GET /danmaku-candidates?title=&ep=&season=
 *   GET /danmaku-pages?bvid=
 *   GET /danmaku-by-bvid?title=&bvid=&out=&threshold=&ep=&cid=&guid=
 *   GET /danmaku-cookie-status
 *   GET /health                     ← 本实现新增：Rust 侧探活/就绪判断
 *
 * 用法：node danmaku-shim.js <uosc_danmaku 脚本目录> [端口=22347]
 */
'use strict';

const http = require('http');
const path = require('path');
const fs = require('fs');
const { URL } = require('url');

const scriptDir = process.argv[2];
const port = parseInt(process.argv[3] || '22347', 10);

if (!scriptDir || !fs.existsSync(path.join(scriptDir, 'bili_danmaku.js'))) {
    console.error('[danmaku-shim] 缺少参数或找不到 bili_danmaku.js: ' + scriptDir);
    process.exit(1);
}

let mod = null;
function loadMod() {
    if (!mod) {
        mod = require(path.join(scriptDir, 'bili_danmaku.js'));
        if (typeof mod.setLogSink === 'function') {
            mod.setLogSink((line) => console.log('[bili_danmaku] ' + line));
        }
    }
    return mod;
}

/** 弹幕 XML 只允许写在 fnos-danmaku 缓存目录（与主仓库 isSafeDanmakuPath 同口径） */
function isSafeDanmakuPath(p) {
    if (!p) return false;
    const norm = path.resolve(p).toLowerCase();
    const pub = (process.env.PUBLIC || 'C:\\Users\\Public').toLowerCase();
    const allowed = [
        path.join(pub, 'fnos-danmaku').toLowerCase(),
        path.join((process.env.ProgramData || 'C:\\ProgramData').toLowerCase(), 'fnos-danmaku').toLowerCase(),
        path.join((process.env.TEMP || '').toLowerCase(), 'fnos-danmaku').toLowerCase(),
    ].filter(Boolean);
    return allowed.some((a) => norm.startsWith(a));
}

function json(res, code, obj) {
    const body = JSON.stringify(obj);
    res.writeHead(code, {
        'Content-Type': 'application/json; charset=utf-8',
        'Content-Length': Buffer.byteLength(body),
    });
    res.end(body);
}

/** 带超时的调用包装：B站接口偶发卡死，别把整个 shim 拖住 */
function withTimeout(promise, ms, label) {
    return Promise.race([
        promise,
        new Promise((_, reject) => setTimeout(() => reject(new Error(label + ' 超时')), ms)),
    ]);
}

/**
 * [lc-1560] 逐源结论 —— 对齐主仓库 `biliRunner.ts` 的 `DanmakuSourceTrace[]`。
 *
 * 消费方是 Lua 的 `modules/menu.lua` → `build_bili_config_menu_props()`（「弹幕源详情」
 * 面板的 ①②③ 三行）与 `src_note()`：
 *   used=true      → "✅ <detail> · N 条"
 *   attempted 未中 → "❌ <error>"
 *   未参与         → "➖ <skippedReason>"
 * 缺了这个数组时 `BILI_INFO.sources` 为 nil，面板退化成「（尚未取过弹幕，无逐源记录）」，
 * 而 ③ 弹弹play 那一行会因 `BILI_INFO.ok==true` 误判为「✅ 已从弹弹play 取到弹幕（前两者
 * 均未命中）」—— 与实际「B站 命中 1931 条」自相矛盾（用户实测截图）。
 *
 * 本应用的源结构与主仓库不同，如实标注：
 *   ① 自建源（danmu_api）：Tauri 侧未接入（主仓库那是 Electron 主进程模块），恒未参与；
 *   ② 内置 B站：本 shim 的唯一实际执行源，used/count/error 取 `run()` 真实结果；
 *   ③ 弹弹play：兜底由 Lua 脚本侧（apis/dandanplay.lua）按需触发，应用侧不参与。
 */
function buildSourceTraces(r) {
    const ok = !!(r && r.ok);
    return [
        {
            id: 'danmu_api',
            attempted: false,
            used: false,
            skippedReason: '本版本未接入自建弹幕接口（Tauri 迁移遗留；弹幕取内置 B站 源）',
        },
        ok
            ? {
                id: 'bilibili',
                attempted: true,
                used: true,
                detail: r.matched_title ? '命中《' + r.matched_title + '》' : '匹配成功',
                count: Number(r.danmaku_count) || 0,
            }
            : {
                id: 'bilibili',
                attempted: true,
                used: false,
                error: (r && r.error) || '未命中',
            },
        {
            id: 'dandanplay',
            attempted: false,
            used: false,
            skippedReason: '弹弹play 兜底由 MPV 弹幕脚本按需触发（应用侧不参与）',
        },
    ];
}

/**
 * [lc-1560] B站 Cookie 体检 → 状态文件写进脚本目录。
 * Lua 的「B站 Cookie 状态」一行直接读 `bili_cookie_status.json`（menu.lua:589）；
 * 主仓库由 playbackShim 每日体检写入，迁移后该文件长期停留在 Electron 时代的旧时间戳
 * （面板「检查于 …」显示的是陈年数据）。shim 启动时刷新一次即可。
 */
async function refreshCookieStatus() {
    try {
        const r = await withTimeout(loadMod().cookie_check(), 30000, 'cookie_check');
        const status = (r && (r.cookie_status || (r.ok ? 'valid' : 'expired'))) || 'unknown';
        const payload = {
            status: status,
            ok: !!(r && r.ok),
            uname: (r && r.uname) || null,
            reason: (r && r.reason) || null,
            checked_at: new Date().toISOString(),
        };
        fs.writeFileSync(
            path.join(scriptDir, 'bili_cookie_status.json'),
            JSON.stringify(payload, null, 2),
            'utf8',
        );
        console.log('[danmaku-shim] Cookie 体检: ' + status + (payload.uname ? ' (' + payload.uname + ')' : ''));
        return payload;
    } catch (e) {
        console.warn('[danmaku-shim] Cookie 体检失败: ' + ((e && e.message) || e));
        return null;
    }
}

async function handle(req, res) {
    const u = new URL(req.url || '/', 'http://127.0.0.1');
    const pathname = u.pathname;
    const q = (k) => (u.searchParams.get(k) || '').toString();

    // 探活：Rust 侧用它判断 shim 是否就绪（不依赖 bili_danmaku.js 加载）
    if (pathname === '/health') {
        json(res, 200, { ok: true, ready: true, pid: process.pid });
        return;
    }

    try {
        if (pathname === '/danmaku') {
            const title = q('title');
            const ep = parseInt(q('ep') || '0', 10) || 0;
            const out = q('out');
            const threshold = q('threshold') ? parseInt(q('threshold'), 10) : undefined;
            const season = q('season') ? parseInt(q('season'), 10) : 0;
            const altTitle = q('alt');
            if (!title || !out) {
                json(res, 400, { ok: false, error: '缺少 title 或 out 参数' });
                return;
            }
            if (!isSafeDanmakuPath(out)) {
                json(res, 403, { ok: false, error: 'out 路径不在允许的弹幕缓存目录内' });
                return;
            }
            console.log(`[danmaku-shim] /danmaku title=${JSON.stringify(title)} ep=${ep} season=${season} out=${out}`);
            const r = await withTimeout(
                loadMod().run(title, ep, out, threshold, season, 60000, true, '', '', altTitle),
                90000,
                '/danmaku',
            );
            if (r && r.ok) {
                json(res, 200, {
                    ok: true,
                    bvid: r.bvid || null,
                    title: r.title || '',
                    matched_title: r.matched_title || '',
                    sim: r.sim == null ? null : r.sim,
                    danmaku_count: r.danmaku_count || 0,
                    source: r.source || '',
                    cid: r.cid == null ? null : r.cid,
                    aggregated_from: r.aggregated_from || null,
                    cookie_status: r.cookie_status || null,
                    season_id: r.season_id == null ? null : r.season_id,
                    epid: r.epid == null ? null : r.epid,
                    season_mismatch: !!r.season_mismatch,
                    // [lc-1560] bili_danmaku.js 自产 sources=false（主仓库由 biliRunner.ts 补），
                    // 这里按同一契约补出来，否则「弹幕源详情」面板的 ①②③ 全是「尚未取过弹幕」。
                    sources: (r.sources && r.sources.length ? r.sources : buildSourceTraces(r)),
                    self_count: r.self_count == null ? null : r.self_count,
                    bilibili_count: r.bilibili_count == null ? null : r.bilibili_count,
                });
            } else {
                json(res, 200, { ok: false, error: (r && r.error) || '未命中', sources: buildSourceTraces(r) });
            }
            return;
        }

        if (pathname === '/danmaku-candidates') {
            const title = q('title');
            const ep = parseInt(q('ep') || '0', 10) || 0;
            const season = q('season') ? parseInt(q('season'), 10) : 0;
            if (!title) {
                json(res, 400, { ok: false, error: '缺少 title 参数' });
                return;
            }
            console.log(`[danmaku-shim] /danmaku-candidates title=${JSON.stringify(title)} ep=${ep}`);
            const r = await withTimeout(
                loadMod().search_candidates(title, ep, season),
                90000,
                '/danmaku-candidates',
            );
            if (r && r.ok) {
                json(res, 200, { ok: true, candidates: r.candidates || [] });
            } else {
                json(res, 200, { ok: false, error: (r && r.error) || '候选搜索失败' });
            }
            return;
        }

        if (pathname === '/danmaku-pages') {
            const bvid = q('bvid');
            if (!bvid) {
                json(res, 400, { ok: false, error: '缺少 bvid 参数' });
                return;
            }
            console.log(`[danmaku-shim] /danmaku-pages bvid=${bvid}`);
            const r = await withTimeout(loadMod().list_pages(bvid), 60000, '/danmaku-pages');
            if (r && r.ok) {
                json(res, 200, { ok: true, pages: r.pages || [] });
            } else {
                json(res, 200, { ok: false, error: (r && r.error) || '分P 获取失败' });
            }
            return;
        }

        if (pathname === '/danmaku-by-bvid') {
            const title = q('title');
            const bvid = q('bvid');
            const out = q('out');
            const threshold = q('threshold') ? parseInt(q('threshold'), 10) : undefined;
            const ep = parseInt(q('ep') || '0', 10) || 0;
            const cid = q('cid');
            if (!bvid || !out) {
                json(res, 400, { ok: false, error: '缺少 bvid 或 out 参数' });
                return;
            }
            if (!isSafeDanmakuPath(out)) {
                json(res, 403, { ok: false, error: 'out 路径不在允许的弹幕缓存目录内' });
                return;
            }
            console.log(`[danmaku-shim] /danmaku-by-bvid bvid=${bvid} ep=${ep} cid=${cid} out=${out}`);
            const r = await withTimeout(
                loadMod().run_candidates(
                    [{ bvid, cid: cid || null, title: title || bvid }],
                    out,
                    threshold,
                    ep,
                ),
                90000,
                '/danmaku-by-bvid',
            );
            if (r && r.ok) {
                json(res, 200, {
                    ok: true,
                    bvid: r.bvid || bvid,
                    title: r.title || '',
                    danmaku_count: r.danmaku_count || 0,
                    source: r.source || '',
                    cid: r.cid == null ? null : r.cid,
                    cookie_status: r.cookie_status || null,
                    sources: buildSourceTraces(r),
                });
            } else {
                json(res, 200, { ok: false, error: (r && r.error) || '未命中', sources: buildSourceTraces(r) });
            }
            return;
        }

        if (pathname === '/danmaku-cookie-status') {
            // [lc-1560] 体检 + 顺带刷新状态文件（面板「B站 Cookie 状态」读它）
            const r = await refreshCookieStatus();
            json(res, 200, r || { status: 'unknown' });
            return;
        }

        json(res, 404, { ok: false, error: '未知端点: ' + pathname });
    } catch (e) {
        const msg = (e && e.message) || String(e);
        console.error('[danmaku-shim] 异常 ' + pathname + ': ' + msg);
        json(res, 500, { ok: false, error: msg });
    }
}

const server = http.createServer((req, res) => {
    handle(req, res).catch((e) => {
        console.error('[danmaku-shim] 顶层异常: ' + ((e && e.message) || e));
        try { json(res, 500, { ok: false, error: String(e) }); } catch (_) {}
    });
});
// 长连接不设超时（弹幕请求可能跑 60s+）
server.keepAliveTimeout = 0;
server.headersTimeout = 0;
server.requestTimeout = 0;

server.on('error', (e) => {
    console.error('[danmaku-shim] 监听失败: ' + ((e && e.message) || e));
    process.exit(2);
});

server.listen(port, '127.0.0.1', () => {
    console.log(`[danmaku-shim] 已启动 127.0.0.1:${port}（脚本目录 ${scriptDir}）`);
    // [lc-1560] 启动即做一次 Cookie 体检（写 bili_cookie_status.json 供 Lua 面板读）。
    // 失败不影响服务（面板会退回"尚未检查"文案）。
    refreshCookieStatus().catch(() => {});
});

// 父进程（Rust）退出时 stdin 关闭 → 跟着退出，不留孤儿。
// 注意：只在**拿到过 stdin**（有父进程管着）时才装这个守卫 —— 手动前台运行
// （stdin 直接是 console）时不该因 EOF 自杀；管道被 PowerShell/& 立即关闭也不该误判。
let stdinSeen = false;
setTimeout(() => { stdinSeen = true; }, 300);
process.stdin.on('end', () => { if (stdinSeen) process.exit(0); });
process.stdin.on('error', () => {});
process.stdin.resume();
