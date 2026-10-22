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
const os = require('os');
const https = require('https');
const crypto = require('crypto');
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
        // [fix-bili-esm] 与 bili-runner.cjs 同一问题：脚本可能位于含 "type": "module"
        // 的目录树下，直接 require(jsPath) 会被当 ESM 解析而失败，统一走 CJS 包装器。
        const deps = require('./danmu-api-deps.cjs');
        mod = deps.loadCjsModule(path.join(scriptDir, 'bili_danmaku.js'));
        if (typeof mod.setLogSink === 'function') {
            mod.setLogSink((line) => console.log('[bili_danmaku] ' + line));
        }
    }
    return mod;
}

/**
 * [lc-1561] 自建弹幕接口（danmu_api）客户端 + 弹幕源编排器 —— 主仓库对应模块的**原样转译产物**。
 *
 * 业务逻辑一字未改：客户端那边是多地址故障转移、只认精确匹配（主名归一化 + 季号一致）、
 * 未标季条目用分集标题核验、系列级匹配记忆、fnOS 原标题备用搜索词；编排器那边是
 * 自建源 → 内置 B站 → 弹弹play 的优先级链、低于聚合阈值时叠加合并、逐源 traces、
 * 候选三源合并与序号重排。全是主仓库实测磨出来的细节，重写必然丢。
 *
 * 配置由 Rust 侧写 %TEMP%/anibox-danmu-config.json + anibox-play-meta.json
 * （见 danmu-api-deps.cjs；每次读取都重读盘 ⇒ 设置面板改完立即生效）。
 */
let runner = null;
function loadRunner() {
    if (!runner) {
        const deps = require('./danmu-api-deps.cjs');
        deps.setScriptDir(scriptDir); // 编排器靠它定位 bili_danmaku.js（替代原 electron 路径探测）
        runner = require('./bili-runner.cjs');
    }
    return runner;
}

/** 系列记忆键：Rust 在 play_item 时把 parent_guid 写进 play-meta.json（省一次 fnOS HTTP） */
function readSeriesKey() {
    try {
        const deps = require('./danmu-api-deps.cjs');
        const c = deps.fnConfig.readConfig();
        return c.parentGuid || c.episodeGuid || '';
    } catch (_) {
        return '';
    }
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

// ===================== [lc-1643] NAS 字幕端点 =====================
// 迁移遗留补齐：Lua 的 fntv_nas_sub.lua 一直调 127.0.0.1:22347 的 /nas-subtitle*，
// 但 Tauri2 迁移时只搬了弹幕端点 ⇒ 「加载字幕」菜单取不到 NAS 外挂字幕、
// 「在线搜索字幕（简体中文）」也永远失败。这里按主仓库
// src/main/common/playbackShim.ts:1087-1240 的契约逐条补齐，请求签名对齐
// src-tauri/src/authx.rs（同一套公开客户端 key/secret，与 Rust 侧完全同源）。

// 签名材料由 Rust 侧拉起时通过环境变量注入（见 danmaku_shim.rs），不硬编码进脚本/产物。
// 未注入 → authxHeader() 返回空串，fnCall() 不加 Authx 头，请求照常发出（网关按未签名处理）。
const FN_API_KEY = (process.env.ANIBOX_FN_AUTHX_KEY || '').trim();
const FN_API_SECRET = (process.env.ANIBOX_FN_AUTHX_SECRET || '').trim();
const SUB_CACHE_DIR = path.join(os.tmpdir(), 'anibox-subtitles');

function md5hex(s) {
    return crypto.createHash('md5').update(s, 'utf8').digest('hex');
}

/** Authx 头：sign = md5(apiKey_path_nonce_ts_md5(body)_apiSecret)，与 authx.rs 逐字节对齐。
 *  未注入签名材料时返回空串（调用方据此不加 Authx 头）。 */
function authxHeader(urlPath, bodyStr) {
    if (!FN_API_KEY || !FN_API_SECRET) return '';
    const nonce = String(Math.floor(Math.random() * 900000) + 100000);
    const ts = Date.now();
    const dataMd5 = md5hex(bodyStr || '');
    const sign = md5hex([FN_API_KEY, urlPath, nonce, ts, dataMd5, FN_API_SECRET].join('_'));
    return `nonce=${nonce}&timestamp=${ts}&sign=${sign}`;
}

/** 服务器地址归一化（对齐 fnapi.rs normalize_server）：补协议头、去引号空白、只留 origin */
function normalizeServer(input) {
    let s = String(input || '').trim().replace(/：/g, ':').replace(/^["'\s]+|["'\s]+$/g, '');
    if (!/^https?:\/\//i.test(s)) s = 'http://' + s;
    const m = s.match(/^(https?:\/\/[^/]+)/i);
    return m ? m[1] : s;
}

/** 拆绝对/相对 Location（对齐 fnapi.rs split_redirect） */
function splitRedirect(base, loc) {
    const i = loc.indexOf('://');
    if (i >= 0) {
        const after = loc.slice(i + 3);
        const slash = after.indexOf('/');
        return slash >= 0
            ? { base: loc.slice(0, i + 3) + after.slice(0, slash), path: after.slice(slash) }
            : { base: loc, path: '/' };
    }
    return { base, path: loc };
}

/**
 * 飞牛 API 调用（二进制安全）：返回 { status, headers, buffer, text, json }。
 * POST/PUT/PATCH 追加 nonce 并以最终序列化字符串签名；3xx 手动跟随并重签新路径；
 * 内网自签证书一律放行（对齐 reqwest accept_invalid_certs）。
 */
function fnCall(base, urlPath, method, token, bodyObj, timeoutMs) {
    const hasBody = /^(post|put|patch)$/i.test(method);
    const bodyStr = hasBody
        ? JSON.stringify(Object.assign({}, bodyObj || {}, { nonce: String(Math.floor(Math.random() * 900000) + 100000) }))
        : '';
    return new Promise((resolve, reject) => {
        const go = (curBase, curPath, depth) => {
            if (depth > 3) return reject(new Error('重定向次数过多'));
            let u;
            try {
                u = new URL(curBase + curPath);
            } catch (e) {
                return reject(new Error('服务器地址格式无效'));
            }
            const mod = u.protocol === 'https:' ? https : http;
            const headers = {
                'Content-Type': 'application/json',
                Authorization: token || '',
                Cookie: token ? `mode=relay; Trim-MC-token=${token}` : 'mode=relay',
                'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) anibox',
                'x-trim-client': 'web',
                'x-trim-client-version': '616',
            };
            // 未注入签名材料时不加 Authx 头（网关按未签名处理）
            const authx = authxHeader(curPath, hasBody ? bodyStr : '');
            if (authx) headers.Authx = authx;
            if (hasBody) headers['Content-Length'] = Buffer.byteLength(bodyStr);
            const req = mod.request(
                {
                    protocol: u.protocol,
                    hostname: u.hostname,
                    port: u.port || (u.protocol === 'https:' ? 443 : 80),
                    path: u.pathname + u.search,
                    method: method.toUpperCase(),
                    headers,
                    rejectUnauthorized: false,
                },
                (resp) => {
                    const st = resp.statusCode || 0;
                    if ([301, 302, 307, 308].includes(st)) {
                        const loc = resp.headers.location || '';
                        resp.resume();
                        if (!loc) return resolve({ status: st, headers: resp.headers, buffer: Buffer.alloc(0), text: '', json: null });
                        const nx = splitRedirect(curBase, loc);
                        return go(nx.base, nx.path, depth + 1);
                    }
                    const chunks = [];
                    resp.on('data', (c) => chunks.push(c));
                    resp.on('end', () => {
                        const buffer = Buffer.concat(chunks);
                        const text = buffer.toString('utf8');
                        let parsed = null;
                        try {
                            parsed = JSON.parse(text);
                        } catch (_) {
                            parsed = null;
                        }
                        resolve({ status: st, headers: resp.headers, buffer, text, json: parsed });
                    });
                },
            );
            req.setTimeout(timeoutMs || 12000, () => req.destroy(new Error('请求超时')));
            req.on('error', reject);
            if (hasBody) req.write(bodyStr);
            req.end();
        };
        go(normalizeServer(base), urlPath, 0);
    });
}

/** 解析 Lua 透传的 fnOS 连接参数；缺参回 400 并返回 null */
function nasParams(res, q) {
    const itemGuid = q('itemGuid');
    const token = q('token');
    const domain = q('domain');
    if (!itemGuid || !token || !domain) {
        json(res, 400, { ok: false, error: '缺少 itemGuid / token / domain 参数' });
        return null;
    }
    return { itemGuid, token, domain };
}

/** GET /v/api/v1/stream/list/{guid} → subtitle_streams（失败返回空数组） */
async function fetchSubtitleStreams(domain, token, itemGuid) {
    const r = await fnCall(domain, `/v/api/v1/stream/list/${encodeURIComponent(itemGuid)}`, 'GET', token, null, 12000);
    if (!r.json || r.json.code !== 0 || !r.json.data) return [];
    return Array.isArray(r.json.data.subtitle_streams) ? r.json.data.subtitle_streams : [];
}

/** 外挂字幕显示名：title 常为空/纯语言标签，退化为「语言 字幕#n」 */
function subTitleOf(s, idx) {
    const lang = String((s && s.language) || '').trim();
    const raw = String((s && s.title) || '').trim();
    return raw || [lang && lang !== 'und' ? lang : '', `字幕#${idx + 1}`].filter(Boolean).join(' ');
}

/** 下载指定 guid 的字幕到本地临时目录（已存在即复用），返回 { path, title } */
async function downloadSubtitleFile(domain, token, guid, name, format) {
    if (!fs.existsSync(SUB_CACHE_DIR)) fs.mkdirSync(SUB_CACHE_DIR, { recursive: true });
    const safeName = String(name || guid).replace(/[\\/:*?"<>|]/g, '_');
    const filePath = path.join(SUB_CACHE_DIR, `${safeName}@${guid}.${format || 'srt'}`);
    if (fs.existsSync(filePath) && fs.statSync(filePath).size > 0) {
        return { path: filePath, title: name || guid };
    }
    const r = await fnCall(domain, `/v/api/v1/subtitle/dl/${encodeURIComponent(guid)}`, 'GET', token, null, 25000);
    const ct = String(r.headers['content-type'] || '').toLowerCase();
    // 服务端出错时会回 JSON 错误体；正常是字幕二进制流
    if (r.json && r.json.code !== 0) throw new Error(r.json.msg || r.json.message || '字幕下载失败');
    if (!r.buffer || r.buffer.length === 0 || ct.includes('application/json')) {
        throw new Error('字幕下载失败(NAS 上不存在或无权限)');
    }
    fs.writeFileSync(filePath, r.buffer);
    return { path: filePath, title: name || guid };
}

/**
 * [lc-1561] 逐源结论的**兜底**生成器。
 *
 * 正常路径下这个数组由移植过来的 `bili-runner.cjs`（= 主仓库 biliRunner.ts 原样转译）
 * 产出的 `r.sources` 提供 —— 自建源/B站 各自的 attempted/used/count/error 都是真实记录。
 * 本函数只在「runner 整体抛异常」时兜底，避免面板退回到「尚未取过弹幕，无逐源记录」。
 *
 * 消费方：Lua 的 `modules/menu.lua` → `build_bili_config_menu_props()` 的 ①②③ 三行。
 *   used=true → "✅ <detail> · N 条"；attempted 未中 → "❌ <error>"；未参与 → "➖ <skippedReason>"
 */
function buildSourceTraces(r) {
    const ok = !!(r && r.ok);
    // 伪 id 直取（手动选定）：编排器不产 traces（单源操作，无优先级链可言），
    // 这里按「谁提供的」如实写，而不是硬套三源模板。
    const src = String((r && r.source) || '');
    if (ok && src.indexOf('自建源') >= 0) {
        return [
            {
                id: 'danmu_api',
                attempted: true,
                used: true,
                detail: '手动选定条目直取',
                count: Number(r.danmaku_count) || 0,
            },
            { id: 'bilibili', attempted: false, used: false, skippedReason: '手动选定自建源条目，未参与' },
            { id: 'dandanplay', attempted: false, used: false, skippedReason: '手动选定自建源条目，未参与' },
        ];
    }
    return [
        {
            id: 'danmu_api',
            attempted: false,
            used: false,
            skippedReason: '本次未走到自建源判定（编排器异常，详见 anibox-danmaku-shim.log）',
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
            skippedReason: '弹弹play 兜底由 MPV 弹幕脚本按需触发（应用侧不含第三方凭证）',
        },
    ];
}

/**
 * 逐源状态归一化（[lc-1660]）：runner 的 traces 只有 attempted/used/detail/error，
 * 失败时 UI 只能笼统显示「尝试过但未命中」。这里为每个源补齐
 * `status`（success / no-result / failed / skipped）、`count`、`error`、`time`，
 * 让「无结果」与「请求失败」在菜单里可区分，并且失败/空结果也能留下逐源记录。
 * 向后兼容：runner 若已带 status 则原样保留；旧字段缺失按 used/attempted 推导。
 */
function looksLikeNoResult(err) {
    const t = String(err || '');
    return /未命中|未找到|未收录|无匹配|无结果|没有匹配|没有找到|0\s*条|为空|未取到/.test(t);
}
function sourceStatusOf(s) {
    if (s && s.status)
        return String(s.status);
    if (!s)
        return 'skipped';
    if (s.used)
        return 'success';
    if (s.attempted)
        return looksLikeNoResult(s.error) ? 'no-result' : 'failed';
    return 'skipped';
}
function normalizeSourceTraces(traces) {
    const now = new Date().toISOString();
    const arr = Array.isArray(traces) ? traces : [];
    const out = [];
    for (const s of arr) {
        if (!s || typeof s !== 'object')
            continue;
        const status = sourceStatusOf(s);
        out.push({
            ...s,
            status,
            count: s.count == null ? 0 : (Number(s.count) || 0),
            error: s.error == null ? null : String(s.error),
            time: s.time || now,
        });
    }
    // 三源缺一不可：runner 某些提前返回分支（如自建源低于阈值但 B站 叠加失败）只推了
    // 部分 traces，这里补齐缺失源，保证 UI 每个源都有明确状态而不是空白。
    const seen = new Set(out.map((s) => s.id));
    for (const id of ['danmu_api', 'bilibili', 'dandanplay']) {
        if (!seen.has(id)) {
            out.push({
                id,
                attempted: false,
                used: false,
                skippedReason: '本轮未参与',
                status: 'skipped',
                count: 0,
                error: null,
                time: now,
            });
        }
    }
    return out;
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

    // ===== [lc-1643] NAS 字幕端点（契约见主仓库 playbackShim.ts:1087-1240）=====
    if (pathname === '/nas-subtitles') {
        const p = nasParams(res, q);
        if (!p) return;
        fetchSubtitleStreams(p.domain, p.token, p.itemGuid)
            .then((streams) => {
                const items = streams
                    .filter((s) => Number(s.extra_file) === 1 || Number(s.is_external) === 1)
                    .map((s, i) => ({
                        guid: String(s.guid || ''),
                        title: subTitleOf(s, i),
                        format: String(s.format || 'srt'),
                        language: String(s.language || '').trim(),
                        is_default: Number(s.is_default) || 0,
                    }));
                console.log(`[danmaku-shim] /nas-subtitles itemGuid=${p.itemGuid} 外挂字幕 ${items.length} 条(流共 ${streams.length} 条)`);
                json(res, 200, { ok: true, items });
            })
            .catch((e) => json(res, 500, { ok: false, error: String((e && e.message) || e) }));
        return;
    }

    if (pathname === '/nas-subtitle-file') {
        const p = nasParams(res, q);
        if (!p) return;
        const guid = q('guid');
        if (!guid) {
            json(res, 400, { ok: false, error: '缺少 guid 参数' });
            return;
        }
        (async () => {
            const streams = await fetchSubtitleStreams(p.domain, p.token, p.itemGuid);
            const idx = streams.findIndex((s) => String(s.guid) === guid);
            const hit = idx >= 0 ? streams[idx] : undefined;
            const name = subTitleOf(hit || {}, idx < 0 ? 0 : idx);
            const out = await downloadSubtitleFile(p.domain, p.token, guid, name, (hit && hit.format) || 'srt');
            console.log(`[danmaku-shim] /nas-subtitle-file guid=${guid} -> ${out.path}`);
            json(res, 200, { ok: true, path: out.path, title: out.title });
        })().catch((e) => json(res, 200, { ok: false, error: String((e && e.message) || e) }));
        return;
    }

    if (pathname === '/nas-subtitle-search') {
        const p = nasParams(res, q);
        if (!p) return;
        const lan = q('lan') || 'zh-CN';
        fnCall(p.domain, '/v/api/v1/subtitle/search', 'POST', p.token, { lan, media_guid: p.itemGuid }, 25000)
            .then((r) => {
                if (!r.json || r.json.code !== 0 || !r.json.data) {
                    json(res, 200, { ok: false, error: (r.json && (r.json.msg || r.json.message)) || 'fnOS 在线字幕搜索失败' });
                    return;
                }
                const items = Array.isArray(r.json.data.subtitles) ? r.json.data.subtitles : [];
                console.log(`[danmaku-shim] /nas-subtitle-search itemGuid=${p.itemGuid} lan=${lan} 候选 ${items.length} 条`);
                json(res, 200, { ok: true, items });
            })
            .catch((e) => json(res, 200, { ok: false, error: String((e && e.message) || e) }));
        return;
    }

    if (pathname === '/nas-subtitle-download') {
        const p = nasParams(res, q);
        if (!p) return;
        const trimId = q('trimId');
        if (!trimId) {
            json(res, 400, { ok: false, error: '缺少 trimId 参数' });
            return;
        }
        (async () => {
            const r = await fnCall(
                p.domain,
                '/v/api/v1/subtitle/download',
                'POST',
                p.token,
                { media_guid: p.itemGuid, trim_id: trimId, sync_download: 1 },
                25000,
            );
            if (!r.json || r.json.code !== 0 || !r.json.data) {
                throw new Error((r.json && (r.json.msg || r.json.message)) || 'fnOS 在线字幕下载失败');
            }
            const s = r.json.data;
            const guid = String(s.guid || '');
            if (!guid) throw new Error('fnOS 未返回新字幕流的 guid');
            const lang = String(s.language || '').trim();
            const name = String(s.title || '').trim() || lang || '在线字幕';
            const out = await downloadSubtitleFile(p.domain, p.token, guid, name, String(s.format || 'srt'));
            console.log(`[danmaku-shim] /nas-subtitle-download trimId=${trimId} guid=${guid} -> ${out.path}`);
            json(res, 200, { ok: true, path: out.path, title: name, guid });
        })().catch((e) => json(res, 200, { ok: false, error: String((e && e.message) || e) }));
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
            const guid = q('guid');
            if (!title || !out) {
                json(res, 400, { ok: false, error: '缺少 title 或 out 参数' });
                return;
            }
            if (!isSafeDanmakuPath(out)) {
                json(res, 403, { ok: false, error: 'out 路径不在允许的弹幕缓存目录内' });
                return;
            }
            console.log(`[danmaku-shim] /danmaku title=${JSON.stringify(title)} ep=${ep} season=${season} guid=${guid || '(无)'} alt=${altTitle ? JSON.stringify(altTitle) : '(无)'}`);
            // [lc-1561] 走移植的编排器（= 主仓库 biliRunner.runBiliDanmaku）：自建源 → 内置 B站
            // → 弹弹play(Lua 侧) 的优先级、低于阈值叠加合并、逐源 traces 全部与原项目一致。
            // seriesKey 由 play-meta.json 直供（Rust 已解析出 parent_guid，省一次 HTTP）。
            const seriesKey = readSeriesKey();
            const r = await withTimeout(
                loadRunner().runBiliDanmaku(title, ep, out, threshold, season, 60000, true, '', seriesKey, altTitle),
                180000,
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
                    // 逐源 traces 由编排器给出（真实记录）；缺省才兜底。统一补齐 status/count/error/time
                    sources: normalizeSourceTraces(r.sources && r.sources.length ? r.sources : buildSourceTraces(r)),
                    self_count: r.self_count == null ? null : r.self_count,
                    bilibili_count: r.bilibili_count == null ? null : r.bilibili_count,
                });
            } else {
                // [lc-1660] 失败/空结果也必须保留 runner 返回的真实逐源记录（含 status/count/error/time），
                // 不能丢弃后只给一条兜底 bilibili 记录，否则 UI 又变成「尚未取过弹幕」。
                json(res, 200, {
                    ok: false,
                    error: (r && r.error) || '未命中',
                    sources: normalizeSourceTraces(r && r.sources && r.sources.length ? r.sources : buildSourceTraces(r)),
                });
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
            // 三源候选合并（自建源置顶 + 弹弹play 段 + B站 兜底），序号重排与原项目一致
            const r = await withTimeout(
                loadRunner().runBiliDanmakuCandidates(title, ep, season, 60000),
                120000,
                '/danmaku-candidates',
            );
            if (r && r.ok) {
                json(res, 200, { ok: true, candidates: r.candidates || [], selfHosted: r.selfHosted || null });
            } else {
                json(res, 200, { ok: false, error: (r && r.error) || '候选搜索失败', selfHosted: r.selfHosted || null });
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
            // [lc-1561] 走编排器的 byBvid（= 主仓库 runBiliDanmakuByBvid）：内部处理
            // 自建源伪 id（dmapi:<id>）/ 弹弹play 伪 id（ddp:<id>）直取、以及 B站 分P 定位。
            const r = await withTimeout(
                loadRunner().runBiliDanmakuByBvid(title || '', bvid, out, threshold, ep, 60000, cid || 0, readSeriesKey()),
                180000,
                '/danmaku-by-bvid',
            );
            if (r && r.ok) {
                json(res, 200, {
                    ok: true,
                    bvid: r.bvid || bvid,
                    title: r.title || '',
                    matched_title: r.matched_title || '',
                    danmaku_count: r.danmaku_count || 0,
                    source: r.source || '',
                    cid: r.cid == null ? null : r.cid,
                    cookie_status: r.cookie_status || null,
                    sources: normalizeSourceTraces(r.sources && r.sources.length ? r.sources : buildSourceTraces(r)),
                });
            } else {
                json(res, 200, {
                    ok: false,
                    error: (r && r.error) || '未命中',
                    sources: normalizeSourceTraces(r && r.sources && r.sources.length ? r.sources : buildSourceTraces(r)),
                });
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
