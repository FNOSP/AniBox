#!/usr/bin/env node
/**
 * [lc-2347] 腾讯视频弹幕源 —— 第四源（与自建 danmu_api / 内置 B站 / 弹弹play 并列叠加）。
 *
 * 链路（两端点均已实测）：
 *   ① 360kan 搜索 `api.so.360kan.com/index?force_v=1&kw=<番名>` → `data.longData.rows[]`
 *      每行 `playlinks.qq` 形如 `https://v.qq.com/x/cover/<cid>/<vid>.html`，
 *      取路径最后一段去 `.html` 即 vid（实测鬼灭柱训练篇 vid=c41007jtl65）。
 *   ② `dm.video.qq.com/barrage/segment/<vid>/t/v1/<startMs>/<endMs>` 纯 JSON
 *      `{ barrage_list: [...] }`，无签名无登录；每段 30s 窗口（实测单段可返回 500+ 条）。
 *      字段：content / time_offset(ms 字符串) / rick_type(>1 = 顶部) / up_count / hot_type。
 *
 * 设计取舍：
 *   - **不碰 360kan 的系列详情接口**（`/v1/detail`，那是 extra.lua 给 360 播放页取源用的）。
 *     这里只用搜索行的 playlinks.qq —— 它已是「某一集的播放页」，vid 直接可用。
 *   - 剧集对位：360kan 每行是「第N季」粒度，同一番会有多行。按标题相似度 + 季号筛选行，
 *     再取该行 playlinks.qq 的 vid。**360kan 不提供集→vid 映射**（一集一个 playlink，
 *     搜索行只给首集），因此本模块只在 vid 已知时可靠；自动取 vid 仅覆盖首集。
 *     这是本源的固有局限，如实写进 trace，不假装能取全季。
 *   - 输出 XML 与 bili_danmaku.js 的 `_write_xml` 完全同格式（`<d p="t,mode,25,color,0,0,0">`），
 *     由 bili-runner 的 writeMergedDanmakuXml 做条目级去重合并。
 */
'use strict';

const https = require('https');
const fs = require('fs');
const path = require('path');
const { URL } = require('url');

const UA = {
    'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36',
};
const S360 = { ...UA };
const VQQ = { ...UA, Referer: 'https://v.qq.com/' };

/**
 * 单段窗口（毫秒）。实测窗口下限 30s（更小直接 404）、上限 120s（300s 也 404），
 * 且**窗口越大条数越多**（60s=1160 条 ≈ 两个 30s 之和 560+600），说明没有单请求条数上限。
 * 取 120s：请求数降到 30s 方案的 1/4，长片（4 集加长版 40min+）也只要 ~20 个请求。
 */
const SEG_MS = 120000;
/** 最多拉多少段（120s × 300 = 10 小时，远超任何单集；实际按连续空段提前停） */
const MAX_SEG = 300;

let _logSink = null;
function setLogSink(fn) { _logSink = (typeof fn === 'function') ? fn : null; }
function log(s) { if (_logSink) { try { _logSink('[tencent] ' + s); } catch (_) { /* 忽略 */ } } }

// ===================== HTTP =====================

function httpGet(urlStr, headers, timeoutMs = 15000) {
    return new Promise((resolve) => {
        let u;
        try { u = new URL(urlStr); } catch (e) { resolve({ ok: false, error: 'URL 非法: ' + e.message }); return; }
        const req = https.request({
            hostname: u.hostname,
            path: u.pathname + u.search,
            method: 'GET',
            headers: headers || {},
            timeout: timeoutMs,
        }, (res) => {
            const chunks = [];
            let n = 0;
            res.on('data', (d) => { n += d.length; if (n <= 8 * 1024 * 1024) chunks.push(d); });
            res.on('end', () => {
                resolve({ ok: res.statusCode >= 200 && res.statusCode < 300, status: res.statusCode, body: Buffer.concat(chunks).toString('utf8') });
            });
        });
        req.on('timeout', () => { req.destroy(); resolve({ ok: false, error: '请求超时(' + timeoutMs + 'ms)' }); });
        req.on('error', (e) => resolve({ ok: false, error: String((e && e.message) || e) }));
        req.end();
    });
}

function jget(urlStr, headers, timeoutMs) {
    return httpGet(urlStr, headers, timeoutMs).then((r) => {
        if (!r.ok) return { err: r.error || ('HTTP ' + r.status) };
        try { return { data: JSON.parse(r.body) }; } catch (e) { return { err: '响应非 JSON: ' + String(r.body).slice(0, 120) }; }
    });
}

// ===================== 标题归一化（与 bili_danmaku.js 同口径） =====================

const _FILLER = ['高清', '1080p', '720p', '480p', '4k', '合集', '全集', '更新', '熟肉', '生肉',
    '国语', '日语', '中字', '双语', '中配', '粤配', 'tv版', '完整版', '未删减', '蓝光', 'bd'];

function normTitle(t) {
    let s = String(t || '').toLowerCase();
    s = s.replace(/<[^>]+>/g, '');           // 360kan 标题带 <b> 高亮
    for (const f of _FILLER) s = s.split(f).join('');
    s = s.replace(/[\s\-_—–·・:：!！?？,，.。'"“”‘’()（）\[\]【】]/g, '');
    return s;
}

function lcsRatio(a, b) {
    if (!a || !b) return 0;
    const m = a.length, n = b.length;
    if (!m || !n) return 0;
    const prev = new Uint16Array(n + 1);
    let best = 0;
    for (let i = 1; i <= m; i++) {
        let diag = 0;
        for (let j = 1; j <= n; j++) {
            const tmp = prev[j];
            if (a[i - 1] === b[j - 1]) { prev[j] = diag + 1; if (prev[j] > best) best = prev[j]; }
            else prev[j] = 0;
            diag = tmp;
        }
    }
    return (2 * best) / (m + n);
}

/**
 * 互相包含优先，否则 LCS 相似度。阈值口径与 bili_danmaku.js 的 title_sim 一致，
 * 但**额外加长度比例惩罚**——纯包含关系在 360kan 上会严重误判：
 *   目标「进击的巨人」(5字) 与候选「无敌巨鲨大战进击巨人」(11字) 互相包含 → 旧口径给 0.95，
 *   实际是 B 级恶搞片（实测拉到 1821 条无关弹幕）。长度比 >1.8 时按比例打折。
 */
function titleSim(a, b) {
    const x = normTitle(a), y = normTitle(b);
    if (!x || !y) return 0;
    if (x === y) return 1;
    // 长度比例惩罚：短串被长串吞掉不等于同片
    const long = Math.max(x.length, y.length);
    const short = Math.min(x.length, y.length);
    const lenRatio = short > 0 ? long / short : Infinity;
    const penalty = lenRatio <= 1.3 ? 1 : (lenRatio <= 1.8 ? 0.85 : (lenRatio <= 2.5 ? 0.6 : 0.35));
    if (x.indexOf(y) >= 0 || y.indexOf(x) >= 0) return 0.95 * penalty;
    return lcsRatio(x, y) * penalty;
}

/**
 * 腾讯源采纳阈值。低于此分不采用（宁可判定「未找到」也不塞错误弹幕）。
 * 0.75 的依据：实测同名/带季后缀的正确命中 ≥0.95；「无敌巨鲨大战进击巨人」这类
 * 误判被长度惩罚压到 0.95*0.35=0.33，远低于阈值，可干净挡掉。
 * 用户仍可在手动候选列表里自行选（list_candidates 不设阈值）。
 */
const ACCEPT_SIM = 0.75;

const _CN_SEASON = { '一': 1, '二': 2, '三': 3, '四': 4, '五': 5, '六': 6, '七': 7, '八': 8, '九': 9, '十': 10 };

/** 从标题解析季号（第N季 / N期 / 第二季 / Season N）。0 = 未标注。 */
function parseSeason(t) {
    const s = String(t || '');
    let m = s.match(/第\s*(\d+)\s*[季部期]/);
    if (m) return parseInt(m[1], 10);
    m = s.match(/第\s*([一二三四五六七八九十])\s*[季部期]/);
    if (m) return _CN_SEASON[m[1]] || 0;
    m = s.match(/[Ss]eason\s*(\d+)/);
    if (m) return parseInt(m[1], 10);
    return 0;
}

// ===================== 360kan：标题 → playlinks.qq =====================

/**
 * 360kan 搜索并按「标题相似度 + 季号」挑出最合适的 qq 播放页。
 * @returns {{ok:boolean, vid?:string, playUrl?:string, matchedTitle?:string, sim?:number, season?:number, candidates?:Array, error?:string}}
 */
async function findQqPlaylink(title, seasonNum) {
    const kw = String(title || '').trim();
    if (!kw) return { ok: false, error: '番名为空' };
    const url = 'https://api.so.360kan.com/index?force_v=1&kw=' + encodeURIComponent(kw);
    const r = await jget(url, S360, 15000);
    if (r.err) return { ok: false, error: '360kan 搜索失败: ' + r.err };
    const j = r.data;
    if (!j || j.code !== 0) return { ok: false, error: '360kan 返回 code=' + (j && j.code) };
    const rows = (j.data && j.data.longData && j.data.longData.rows) || [];
    if (!rows.length) return { ok: false, error: '360kan 无搜索结果' };

    // 只保留有 qq 播放页的行
    const qqRows = [];
    for (const row of rows) {
        const pl = row && row.playlinks;
        const qqUrl = pl && pl.qq;
        if (!qqUrl || typeof qqUrl !== 'string') continue;
        const raw = String(row.titleTxt || row.title || '').replace(/<[^>]+>/g, '');
        const sim = titleSim(kw, raw);
        const season = parseSeason(raw) || parseSeason(kw);
        qqRows.push({ row, raw, sim, season, qqUrl });
    }
    if (!qqRows.length) {
        return { ok: false, error: '360kan 无腾讯播放页（该番腾讯未上架）', candidates: rows.slice(0, 6).map((x) => String(x.titleTxt || x.title || '')) };
    }

    // 排序：季命中优先 → 相似度 → 有 seriesPlaylinks（整季）优先
    qqRows.sort((a, b) => {
        const sa = (seasonNum > 0 && a.season === seasonNum) ? 0 : (seasonNum > 0 && a.season > 0 ? 2 : 1);
        const sb = (seasonNum > 0 && b.season === seasonNum) ? 0 : (seasonNum > 0 && b.season > 0 ? 2 : 1);
        if (sa !== sb) return sa - sb;
        if (b.sim !== a.sim) return b.sim - a.sim;
        return 0;
    });

    const best = qqRows[0];
    // 相似度门控：宁可不给弹幕，也不给错片的弹幕（错弹幕比没弹幕更伤）
    if (best.sim < ACCEPT_SIM) {
        log(`最优候选「${best.raw}」sim=${best.sim.toFixed(2)} < 阈值${ACCEPT_SIM} → 不采用（避免错配）`);
        return {
            ok: false,
            error: `腾讯《${best.raw}》相似度仅 ${best.sim.toFixed(2)}（阈值 ${ACCEPT_SIM}），疑似非同片，不采用`,
            candidates: qqRows.map((x) => `${x.raw}${x.season ? ` (第${x.season}季)` : ''} sim=${x.sim.toFixed(2)}`),
        };
    }
    const vid = extractVid(best.qqUrl);
    if (!vid) {
        return { ok: false, error: '360kan 播放页 URL 里取不到 vid: ' + best.qqUrl, candidates: qqRows.map((x) => x.raw) };
    }
    log(`360kan 命中「${best.raw}」sim=${best.sim.toFixed(2)} season=${best.season} vid=${vid}`);
    return {
        ok: true,
        vid: vid,
        playUrl: best.qqUrl,
        matchedTitle: best.raw,
        sim: best.sim,
        season: best.season,
        candidates: qqRows.map((x) => x.raw + (x.season ? ` (第${x.season}季)` : '')),
    };
}

/**
 * 从腾讯播放页 URL 取 vid。
 *   https://v.qq.com/x/cover/mzc00200g760kt3/l4101am1l1o.html?traceid=...  → l4101am1l1o
 *   https://v.qq.com/x/page/l4101am1l1o.html                              → l4101am1l1o
 *   https://v.qq.com/x/cover/<cid>.html（整季页，无单集 vid）              → 抛给调用方判空
 */
function extractVid(u) {
    const s = String(u || '');
    // /x/cover/<cid>/<vid>.html
    let m = s.match(/\/x\/cover\/[^/]+\/([A-Za-z0-9]+)\.html/);
    if (m) return m[1];
    // /x/page/<vid>.html
    m = s.match(/\/x\/page\/([A-Za-z0-9]+)\.html/);
    if (m) return m[1];
    // 兜底：最后一个形如 l4101am1l1o / c41007jtl65 的段
    m = s.match(/\/([a-z]\d{4}[a-z0-9]{4,})\.html/i);
    if (m) return m[1];
    return '';
}

// ===================== 腾讯弹幕拉取 =====================

/** 拉一段 30s 窗口。返回 [{t(秒), text, mode, color}]，失败返回 []。 */
async function fetchSegment(vid, startMs) {
    const url = `https://dm.video.qq.com/barrage/segment/${encodeURIComponent(vid)}/t/v1/${startMs}/${startMs + SEG_MS}`;
    const r = await jget(url, VQQ, 12000);
    if (r.err) { log(`段 ${startMs} 失败: ${r.err}`); return []; }
    const list = (r.data && r.data.barrage_list) || [];
    const out = [];
    for (const it of list) {
        const text = it && it.content != null ? String(it.content) : '';
        if (!text) continue;
        const off = parseInt(it.time_offset, 10);
        const t = (isNaN(off) ? startMs : off) / 1000;
        // rick_type > 1 → 顶部弹幕（AniCh 的 getQQPosition 同口径）；其余滚动
        const mode = (parseInt(it.rick_type, 10) || 0) > 1 ? 5 : 1;
        out.push({ t: t, text: text, mode: mode, color: 16777215 });
    }
    return out;
}

/**
 * 拉整集弹幕。按 30s 分段顺序拉，连续 2 段空即提前停（末段特征）。
 * @returns {{ok:boolean, list?:Array, error?:string, segments?:number}}
 */
async function fetchAll(vid, maxMs) {
    if (!vid) return { ok: false, error: 'vid 为空' };
    const hardStop = Math.min((maxMs || (MAX_SEG * SEG_MS)), MAX_SEG * SEG_MS);
    const all = [];
    let emptyStreak = 0;
    let seg = 0;
    for (let start = 0; start < hardStop; start += SEG_MS) {
        const part = await fetchSegment(vid, start);
        seg++;
        if (!part.length) {
            emptyStreak++;
            // 连续 2 段空 → 认为已到末尾（单段偶发空可能是风控，不值得为它拉满 5 小时）
            if (emptyStreak >= 2) break;
        } else {
            emptyStreak = 0;
            for (const d of part) all.push(d);
        }
    }
    if (!all.length) return { ok: false, error: '腾讯未返回任何弹幕（vid 可能无弹幕池）', segments: seg };
    log(`vid=${vid} 拉到 ${all.length} 条，扫了 ${seg} 段`);
    return { ok: true, list: all, segments: seg };
}

// ===================== 输出 XML（与 bili_danmaku.js _write_xml 完全同格式） =====================

function htmlEscape(s) {
    return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

function writeXml(out, dm) {
    const dir = path.dirname(out) || '.';
    fs.mkdirSync(dir, { recursive: true });
    const lines = ['<?xml version="1.0" encoding="UTF-8"?>', '<danmaku>'];
    for (const d of dm) {
        // p = "时间(秒),模式,字号,颜色,0,0,0" —— 与 B站 侧一致（字号 25 = libass 默认）
        const p = `${d.t.toFixed(2)},${d.mode},25,${d.color},0,0,0`;
        lines.push(`<d p="${p}">${htmlEscape(d.text)}</d>`);
    }
    lines.push('</danmaku>');
    fs.writeFileSync(out, lines.join('\n') + '\n', 'utf8');
    return dm.length;
}

// ===================== 对外主入口 =====================

/**
 * 端到端：番名(+季) → vid → 弹幕 → 写 XML。
 * @param {string} title  番名
 * @param {number} season 季号（0 = 不筛）
 * @param {string} out    输出 XML 路径
 * @param {number} timeoutMs 全局超时
 * @param {string} vidHint 已知 vid 时直接用它（跳过 360kan 搜索；手动选定场景）
 */
async function run(title, season, out, timeoutMs, vidHint) {
    const started = Date.now();
    const budget = timeoutMs || 90000;
    const remain = () => Math.max(1000, budget - (Date.now() - started));

    let vid = vidHint ? String(vidHint).trim() : '';
    let matchedTitle = '';
    let sim = null;
    let seasonHit = false;
    let source = 'tencent';
    let candidates = null;

    if (!vid) {
        const found = await Promise.race([
            findQqPlaylink(title, Number(season) || 0),
            new Promise((r) => setTimeout(() => r({ ok: false, error: '360kan 搜索超时' }), Math.min(20000, remain()))),
        ]);
        if (!found.ok) {
            return {
                ok: false, error: found.error || '未找到腾讯播放页',
                source: source, candidates: found.candidates || null,
                video_found: false,
            };
        }
        vid = found.vid;
        matchedTitle = found.matchedTitle || '';
        sim = typeof found.sim === 'number' ? found.sim : null;
        seasonHit = Number(season) > 0 && found.season === Number(season);
        candidates = found.candidates || null;
    } else {
        matchedTitle = title || '';
        source = 'tencent-manual';
    }

    const res = await Promise.race([
        fetchAll(vid, null),
        new Promise((r) => setTimeout(() => r({ ok: false, error: '弹幕拉取超时(' + budget + 'ms)' }), remain())),
    ]);
    if (!res.ok) {
        return {
            ok: false, error: res.error || '腾讯弹幕为空',
            source: source, vid: vid, matched_title: matchedTitle, sim: sim,
            video_found: true, candidates: candidates,
        };
    }

    const count = writeXml(out, res.list);
    log(`✅ 腾讯弹幕 ${count} 条 -> ${out} (vid=${vid} ${matchedTitle})`);
    return {
        ok: true,
        vid: vid,
        title: title,
        matched_title: matchedTitle || title,
        sim: sim,
        danmaku_count: count,
        source: source,
        season_match: seasonHit,
        video_found: true,
        candidates: candidates,
        segments: res.segments || null,
    };
}

/** 仅搜索（供手动候选列表展示），不拉弹幕。 */
async function searchCandidates(title, season) {
    const found = await findQqPlaylink(title, Number(season) || 0);
    if (!found.ok) return { ok: false, error: found.error };
    return {
        ok: true,
        vid: found.vid,
        matched_title: found.matchedTitle,
        sim: found.sim,
        season: found.season,
        play_url: found.playUrl,
        candidates: found.candidates,
    };
}

module.exports = {
    run: run,
    search_candidates: searchCandidates,
    find_qq_playlink: findQqPlaylink,
    extract_vid: extractVid,
    fetch_all: fetchAll,
    write_xml: writeXml,
    setLogSink: setLogSink,
    // 供单测
    _internal: { titleSim: titleSim, parseSeason: parseSeason, normTitle: normTitle },
};
