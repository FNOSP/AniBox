#!/usr/bin/env node
/**
 * [lc-2351] 优酷弹幕源 —— 第六源。
 *
 * ## 链路（全链路实测通过，2026-11-24）
 *
 *   ① **cna cookie**：`https://log.mmstat.com/eg.js` 的 `Set-Cookie: cna=...`
 *      （优酷把设备指纹放在这个阿里统计 cookie 里，不需要登录）
 *   ② **_m_h5_tk**：`https://acs.youku.com/h5/mtop.com.youku.aplatform.weakget/1.0/?jsv=2.5.1&appKey=24679788`
 *      返回 `_m_h5_tk`（形如 `<32位hex>_<时间戳>`）与 `_m_h5_tk_enc`
 *   ③ **视频信息**：`https://openapi.youku.com/v2/videos/show.json?client_id=53e6cc67237fc59a&...&video_id=<vid>`
 *      → `title` / `duration`
 *   ④ **弹幕**：`POST https://acs.youku.com/h5/mopen.youku.danmu.list/1.0/?<签名参数>`
 *      body = `data=<外层JSON>`（URL 编码）
 *
 * ## 签名（照抄 KikoPlayScript danmu/youku.lua，MIT，逐行核对过）
 *
 * ```
 * inner  = {ctime, ctype:10004, cver:'v1.0', guid:cna, mat:seg, mcount:1,
 *           pid:0, sver:'3.1.0', type:1, vid}
 * msg_b64 = base64(JSON(inner))
 * inner.msg  = msg_b64
 * inner.sign = md5(msg_b64 + "MkmC9SoIw6xCkSKHhJ7b5D2r51kBiREr")     ← 签 base64 串
 * data = JSON(inner)                                                   ← **外层 JSON**（含 msg+sign）
 * t = 当前毫秒
 * t_sign = md5(tk前32位 + "&" + t + "&" + "24679788" + "&" + data)      ← 签外层 JSON
 * POST data=<urlencode(data)>
 * Query: jsv/appKey/t/sign/api/v/type/dataType/timeout/jsonpIncPrefix
 * Cookie: _m_h5_tk=<tk>; _m_h5_tk_enc=<enc>; cna=<cna>
 * ```
 *
 * **踩过的两个坑**（都已修正，别再犯）：
 *   1. `data` 不是 base64 串，而是**含 msg+sign 的外层 JSON**。第一版把 data 当 base64
 *      直接 POST → `FAIL_SYS_TOKEN_EMPTY::令牌为空`（服务端认不出参数结构）。
 *   2. `t_sign` 签的也是**外层 JSON**（不是 base64 串）。
 *
 * ## 字段映射
 *
 * 响应 `data.result` 是**字符串化的 JSON**（需二次 parse），内层 `data.result[]`：
 *   - `content`    弹幕文本
 *   - `playat`     **毫秒**
 *   - `propertis`  也是字符串化 JSON：`{color:<int>, size, pos, effect, markSource}`
 *   - `uid2`       用户 id（做去重/标注用）
 *
 * `mat` 是**分钟段号**（从 1 开始）：`segs = floor(duration / 60) + 1`。
 *
 * ## 局限（如实标注）
 *
 *   - **没有搜索接口**：openapi 只接受已知 vid，优酷的搜索页需要额外签名。
 *     因此本源**只能手动指定 vid**（或由上层传入），不能像爱奇艺/腾讯那样按番名自动匹配。
 *     这是本源当前的真实能力边界，不假装能自动搜。
 *   - `cna`/`_m_h5_tk` 每次都新取（无缓存），多集连播会多两次 HTTP（实测各 ~200ms）。
 */
'use strict';

const https = require('https');
const crypto = require('crypto');
const fs = require('fs');
const path = require('path');
const { URL } = require('url');

const UA = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) Gecko/20100101 Firefox/108.0';
const APP_KEY = '24679788';
/** 优酷 H5 端公开的签名盐（来自 KikoPlayScript danmu/youku.lua，MIT）。 */
const SECRET = 'MkmC9SoIw6xCkSKHhJ7b5D2r51kBiREr';
const OPENAPI_CLIENT = '53e6cc67237fc59a';
const OPENAPI_PACKAGE = 'com.huawei.hwvplayer.youku';

let _logSink = null;
function setLogSink(fn) { _logSink = (typeof fn === 'function') ? fn : null; }
function log(s) { if (_logSink) { try { _logSink('[youku] ' + s); } catch (_) { /* 忽略 */ } } }

const md5 = (s) => crypto.createHash('md5').update(s, 'utf8').digest('hex');

// ===================== HTTP =====================

function req(urlStr, { method = 'GET', headers = {}, body = null, timeout = 15000 } = {}) {
    return new Promise((resolve) => {
        let u;
        try { u = new URL(urlStr); } catch (e) { resolve({ ok: false, error: 'URL 非法: ' + e.message }); return; }
        const lib = u.protocol === 'http:' ? require('http') : https;
        const r = lib.request({
            hostname: u.hostname,
            path: u.pathname + u.search,
            method,
            headers: Object.assign({ 'User-Agent': UA }, headers),
            timeout,
        }, (s) => {
            const c = [];
            let n = 0;
            s.on('data', (d) => { n += d.length; if (n <= 16 * 1024 * 1024) c.push(d); });
            s.on('end', () => resolve({
                ok: s.statusCode >= 200 && s.statusCode < 300,
                status: s.statusCode,
                headers: s.headers,
                buf: Buffer.concat(c),
            }));
        });
        r.on('timeout', () => { r.destroy(); resolve({ ok: false, error: '请求超时(' + timeout + 'ms)' }); });
        r.on('error', (e) => resolve({ ok: false, error: String((e && e.message) || e) }));
        if (body) r.write(body);
        r.end();
    });
}

function parseCookies(setCookie) {
    const out = {};
    const arr = Array.isArray(setCookie) ? setCookie : (setCookie ? [setCookie] : []);
    for (const row of arr) {
        const first = String(row).split(';')[0];
        const i = first.indexOf('=');
        if (i > 0) out[first.slice(0, i).trim()] = first.slice(i + 1).trim();
    }
    return out;
}

// ===================== 前置凭证 =====================

/**
 * 取 cna（阿里统计设备指纹 cookie）与 _m_h5_tk（优酷 mtop 令牌）。
 * 两者都是**匿名**可得的，不需要登录。
 * @returns {{ok:boolean, cna?:string, tk?:string, tkEnc?:string, error?:string}}
 */
async function fetchTokens() {
    const r1 = await req('https://log.mmstat.com/eg.js', {
        headers: { 'Accept-Encoding': 'gzip, deflate', Accept: '*/*', Connection: 'keep-alive' },
    });
    if (!r1.ok) return { ok: false, error: '取 cna 失败: ' + (r1.error || ('HTTP ' + r1.status)) };
    const cna = parseCookies(r1.headers && r1.headers['set-cookie']).cna;
    if (!cna) return { ok: false, error: '取 cna 失败：响应无 cna cookie' };

    const r2 = await req(`https://acs.youku.com/h5/mtop.com.youku.aplatform.weakget/1.0/?jsv=2.5.1&appKey=${APP_KEY}`);
    if (!r2.ok) return { ok: false, error: '取 _m_h5_tk 失败: ' + (r2.error || ('HTTP ' + r2.status)) };
    const ck = parseCookies(r2.headers && r2.headers['set-cookie']);
    if (!ck['_m_h5_tk']) return { ok: false, error: '取 _m_h5_tk 失败：响应无该 cookie' };
    return { ok: true, cna: cna, tk: ck['_m_h5_tk'], tkEnc: ck['_m_h5_tk_enc'] || '' };
}

/** 从优酷播放页 URL 提 vid（形如 v.youku.com/v_show/id_XNTkxMjQxNzM4MA==.html）。 */
function extractVid(u) {
    const m = String(u || '').match(/v_show\/id_([A-Za-z0-9=+/]+)\.html/);
    return m ? m[1] : '';
}

// ===================== 视频信息 =====================

/** 取标题与时长（秒）。openapi 是公开接口，不需要签名。
 *
 *  注意 `duration` 是**字符串**（实测 `"1434.56"`）不是数字 —— KikoPlayScript 用
 *  `tonumber()` 转，这里必须 `parseFloat`，否则 `typeof !== 'number'` 会被误判为无效 vid。 */
async function videoInfo(vid) {
    const url = 'https://openapi.youku.com/v2/videos/show.json?client_id=' + OPENAPI_CLIENT +
        '&package=' + OPENAPI_PACKAGE + '&ext=show&video_id=' + encodeURIComponent(vid);
    const r = await req(url);
    if (!r.ok) return { ok: false, error: 'openapi 请求失败: ' + (r.error || ('HTTP ' + r.status)) };
    let j;
    try { j = JSON.parse(r.buf.toString('utf8')); } catch (e) { return { ok: false, error: 'openapi 响应非 JSON' }; }
    const duration = parseFloat(j.duration);
    if (!isFinite(duration) || duration <= 0) {
        return { ok: false, error: 'openapi 未返回有效时长（vid 可能无效）: ' + JSON.stringify(j).slice(0, 120) };
    }
    return { ok: true, title: j.title || '', duration: duration };
}

// ===================== 弹幕拉取 =====================

/** propertis 是字符串化 JSON：{color:<int>, size, pos, effect} */
function parseProps(s) {
    try {
        const o = typeof s === 'string' ? JSON.parse(s) : (s || {});
        return {
            color: (typeof o.color === 'number' && o.color >= 0 && o.color <= 0xFFFFFF) ? o.color : 16777215,
            pos: typeof o.pos === 'number' ? o.pos : 0,
            effect: typeof o.effect === 'number' ? o.effect : 0,
        };
    } catch (e) {
        return { color: 16777215, pos: 0, effect: 0 };
    }
}

/**
 * 拉一个分钟段。`seg` 从 1 开始，对应 `mat` 字段。
 * @returns {{ok:boolean, list?:Array, code?:number, error?:string}}
 */
async function fetchSegment(tokens, vid, seg) {
    const inner = {
        // ctime 用**毫秒字符串**（实测：传数字也能过，但 Lua 版是 tostring，保持一致）
        ctime: String(Date.now()),
        ctype: 10004,
        cver: 'v1.0',
        guid: tokens.cna,
        mat: seg,
        mcount: 1,
        pid: 0,
        sver: '3.1.0',
        type: 1,
        vid: vid,
    };
    const msgB64 = Buffer.from(JSON.stringify(inner), 'utf8').toString('base64');
    inner.msg = msgB64;
    inner.sign = md5(msgB64 + SECRET);
    // data 是**含 msg+sign 的外层 JSON**（不是 base64 串——这一点踩过一次）
    const data = JSON.stringify(inner);

    const t = String(Date.now());
    const tkPrefix = String(tokens.tk).split('_')[0]; // 前 32 位 hex
    const tSign = md5(`${tkPrefix}&${t}&${APP_KEY}&${data}`);

    const qs = new URLSearchParams({
        jsv: '2.5.6',
        appKey: APP_KEY,
        t: t,
        sign: tSign,
        api: 'mopen.youku.danmu.list',
        v: '1.0',
        type: 'originaljson',
        dataType: 'jsonp',
        timeout: '20000',
        jsonpIncPrefix: 'utility',
    });
    const cookieHeader = `_m_h5_tk=${tokens.tk}; _m_h5_tk_enc=${tokens.tkEnc}; cna=${tokens.cna}`;

    const r = await req('https://acs.youku.com/h5/mopen.youku.danmu.list/1.0/?' + qs.toString(), {
        method: 'POST',
        headers: {
            'Content-Type': 'application/x-www-form-urlencoded',
            Cookie: cookieHeader,
            Referer: 'https://v.youku.com',
        },
        body: 'data=' + encodeURIComponent(data),
    });
    if (!r.ok) return { ok: false, error: '弹幕请求失败: ' + (r.error || ('HTTP ' + r.status)) };

    let outer;
    try { outer = JSON.parse(r.buf.toString('utf8')); } catch (e) {
        return { ok: false, error: '弹幕响应非 JSON: ' + r.buf.toString('utf8').slice(0, 100) };
    }
    // mtop 的业务错误放在 ret 数组里（如 FAIL_SYS_TOKEN_EMPTY）
    if (Array.isArray(outer.ret) && outer.ret.length && /^FAIL_/.test(String(outer.ret[0]))) {
        return { ok: false, error: '优酷返回错误: ' + outer.ret.join(',') };
    }
    // data.result 是**字符串化的 JSON**（需二次 parse）
    let mid;
    try {
        mid = typeof outer.data.result === 'string' ? JSON.parse(outer.data.result) : outer.data.result;
    } catch (e) {
        return { ok: false, error: '弹幕 result 二次解析失败' };
    }
    if (!mid || mid.code !== 1 || !mid.data || !Array.isArray(mid.data.result)) {
        // code !== 1 通常是该段无弹幕（末尾），不是错误
        return { ok: true, list: [], code: mid && mid.code };
    }
    const out = [];
    for (const dm of mid.data.result) {
        const text = dm && dm.content != null ? String(dm.content) : '';
        if (!text) continue;
        const ms = parseInt(dm.playat, 10);
        if (!isFinite(ms) || ms < 0) continue;
        const p = parseProps(dm.propertis);
        // pos: 实测 3 = 滚动；顶部/底部按优酷 H5 约定（pos=1 顶部 / pos=2 底部），
        // 未在本次样本中出现，保守按滚动处理（错判成顶部会遮挡画面，宁可滚动）
        let mode = 1;
        if (p.pos === 1) mode = 5;
        else if (p.pos === 2) mode = 4;
        out.push({ t: ms / 1000, text: text, mode: mode, color: p.color });
    }
    return { ok: true, list: out, code: mid.code };
}

/**
 * 拉整集：按分钟段递增，连续 2 段空即停。
 * @returns {{ok:boolean, list?:Array, segments?:number, error?:string}}
 */
async function fetchAll(tokens, vid, durationSec, maxSeg) {
    if (!vid) return { ok: false, error: 'vid 为空' };
    const total = Math.floor((Number(durationSec) || 0) / 60) + 1;
    const limit = Math.min(maxSeg || total, Math.max(total, 1), 600);
    const all = [];
    let emptyStreak = 0;
    let seg = 0;
    for (let i = 1; i <= limit; i++) {
        const r = await fetchSegment(tokens, vid, i);
        seg = i;
        if (!r.ok) {
            // 鉴权类错误直接中止（重试也不会好）；其它当空段处理
            if (/TOKEN|SIGN|FAIL_/.test(String(r.error || ''))) {
                log(`段${i} 鉴权失败，中止: ${r.error}`);
                if (!all.length) return { ok: false, error: r.error, segments: seg };
                break;
            }
            emptyStreak++;
            if (emptyStreak >= 2) break;
            continue;
        }
        if (!r.list || !r.list.length) {
            emptyStreak++;
            if (emptyStreak >= 2) break;
        } else {
            emptyStreak = 0;
            for (const d of r.list) all.push(d);
        }
    }
    if (!all.length) return { ok: false, error: '优酷未返回任何弹幕（该视频可能无弹幕池）', segments: seg };
    log(`vid=${vid} 拉到 ${all.length} 条，扫了 ${seg} 段（总段数 ${total}）`);
    return { ok: true, list: all, segments: seg };
}

// ===================== 输出 XML =====================

function htmlEscape(s) {
    return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

function writeXml(out, dm) {
    const dir = path.dirname(out) || '.';
    fs.mkdirSync(dir, { recursive: true });
    const lines = ['<?xml version="1.0" encoding="UTF-8"?>', '<danmaku>'];
    for (const d of dm) {
        const p = `${d.t.toFixed(2)},${d.mode},25,${d.color},0,0,0`;
        lines.push(`<d p="${p}">${htmlEscape(d.text)}</d>`);
    }
    lines.push('</danmaku>');
    fs.writeFileSync(out, lines.join('\n') + '\n', 'utf8');
    return dm.length;
}

// ===================== 对外主入口 =====================

/**
 * 端到端：vid → 弹幕 → 写 XML。
 *
 * **本源只能按 vid 拉，不能按番名搜**（优酷搜索接口需额外签名，未实现）。
 * @param {string} vid 优酷 vid（`XNTkxMjQxNzM4MA==` 这种）或播放页 URL（自动提取）
 * @param {string} out 输出 XML 路径
 * @param {number} timeoutMs 全局超时
 */
async function run(vidOrUrl, out, timeoutMs) {
    const started = Date.now();
    const budget = timeoutMs || 90000;
    const remain = () => Math.max(1000, budget - (Date.now() - started));

    let vid = String(vidOrUrl || '').trim();
    if (!vid) return { ok: false, error: 'vid/URL 为空', source: 'youku' };
    if (vid.indexOf('v_show') >= 0 || vid.indexOf('youku.com') >= 0) {
        const extracted = extractVid(vid);
        if (!extracted) return { ok: false, error: '从 URL 提取 vid 失败: ' + vid, source: 'youku' };
        vid = extracted;
    }

    const tokens = await fetchTokens();
    if (!tokens.ok) return { ok: false, error: tokens.error, source: 'youku', vid: vid };

    const info = await Promise.race([
        videoInfo(vid),
        new Promise((r) => setTimeout(() => r({ ok: false, error: '视频信息超时' }), Math.min(15000, remain()))),
    ]);
    if (!info.ok) return { ok: false, error: info.error, source: 'youku', vid: vid };

    const res = await Promise.race([
        fetchAll(tokens, vid, info.duration, null),
        new Promise((r) => setTimeout(() => r({ ok: false, error: '弹幕拉取超时(' + budget + 'ms)' }), remain())),
    ]);
    if (!res.ok) {
        return {
            ok: false, error: res.error || '优酷弹幕为空',
            source: 'youku', vid: vid, matched_title: info.title || '',
            video_found: true, segments: res.segments || null,
        };
    }

    const count = writeXml(out, res.list);
    log(`✅ 优酷弹幕 ${count} 条 -> ${out} (vid=${vid} ${info.title})`);
    return {
        ok: true,
        vid: vid,
        title: info.title || '',
        matched_title: info.title || '',
        duration: info.duration,
        danmaku_count: count,
        source: 'youku',
        video_found: true,
        segments: res.segments || null,
    };
}

module.exports = {
    run: run,
    extract_vid: extractVid,
    fetch_tokens: fetchTokens,
    video_info: videoInfo,
    fetch_all: fetchAll,
    write_xml: writeXml,
    setLogSink: setLogSink,
    // 供单测：本源目前没有标题匹配逻辑（不能按番名搜），故不导出 titleSim
    _internal: { parseProps: parseProps },
};
