#!/usr/bin/env node
/**
 * [lc-2350] 爱奇艺弹幕源 —— 第五源。
 *
 * ## 链路（两端点均实测，2026-11-24）
 *
 *   ① 搜索：`https://search.video.iqiyi.com/o?key=<番名>&if=html5`
 *      → `data.docinfos[].albumDocInfo.videoinfos[]`，每项含 `tvId`/`itemshortTitle`/`timeLength`
 *   ② 弹幕：`https://cmts.iqiyi.com/bullet/<s1>/<s2>/<tvid>_300_<n>.z`
 *      其中 `s1 = ("0000"+tvid).slice(-4,-2)`、`s2 = ("0000"+tvid).slice(-2)`
 *      响应是 **zlib 压缩的 XML**（`zlib.inflateSync` 直接可解，Node 内置）
 *
 * **完全公开：无签名、无登录、无 Cookie。** 实测：
 *   - 狂飙第1集 tvid=3493131456125200 → 18265 条 / 10 段
 *   - 琅琊榜第1集 tvid=399163400      → 14933 条 / 9 段
 *
 * ## XML 结构与字段（逐项实测，别凭印象）
 *
 * ```xml
 * <danmu><code>A00000</code><data><entry><int>1</int><list>
 *   <bulletInfo>
 *     <content>京海不大，创造神话</content>   ← 弹幕文本
 *     <showTime>1</showTime>                  ← **秒**（不是毫秒！段 N 覆盖 (N-1)*300 ~ N*300）
 *     <color>ffffff</color>                   ← hex 字符串，**有大小写混杂**（fffFFF 都出现过）
 *     <position>0</position>                  ← 位置，见下方映射
 *     <font>14</font>
 *   </bulletInfo>
 *   ...
 * ```
 *
 * `position` 实测分布（狂飙首段 1947 条）：
 *   - `0`   → 1775 条，滚动（默认）
 *   - `1`   → 145 条，顶部
 *   - `200` → 7 条，底部
 *   - `100`/`108` → 高级弹幕（变色/移动特效），**丢弃**（我们 ASS 渲染不支持，硬套会错位）
 *
 * ## 与腾讯源的差异（为什么独立文件而不是复用）
 *
 *   - 腾讯按 **120s 窗口的时间区间**拉（`/t/v1/<start>/<end>`），爱奇艺按 **300s 的段序号**拉
 *     （`_300_<n>.z`）—— 分片模型不同，窗口逻辑无法共用。
 *   - 腾讯返回 JSON，爱奇艺返回 zlib+XML —— 解析器完全不同。
 *   - 腾讯 `time_offset` 是毫秒，爱奇艺 `showTime` 是秒 —— 单位不同，混用必错。
 *
 * 共同点只在外层：都产出 `[{t(秒), text, mode, color(int)}]` 给同一个 writeXml。
 * 因此 `writeXml` / `titleSim` / `parseSeason` 这套从 tencent-danmaku.cjs 复制的部分
 * 保持**逐字一致**（utils 层将来可抽公共模块，现在两处各留一份更易读）。
 */
'use strict';

const https = require('https');
const fs = require('fs');
const path = require('path');
const zlib = require('zlib');
const { URL } = require('url');

const UA = {
    'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36',
};
const IQIYI = { ...UA, Referer: 'https://www.iqiyi.com/' };

/** 每段覆盖的秒数（实测：段 1 = 0~300s，段 2 = 301~600s）。 */
const SEG_SECONDS = 300;
/** 最多拉多少段（300s × 40 = 200 分钟，远超任何单集；实际按空段提前停）。 */
const MAX_SEG = 40;

let _logSink = null;
function setLogSink(fn) { _logSink = (typeof fn === 'function') ? fn : null; }
function log(s) { if (_logSink) { try { _logSink('[iqiyi] ' + s); } catch (_) { /* 忽略 */ } } }

// ===================== HTTP =====================

function httpGetBuffer(urlStr, headers, timeoutMs = 15000) {
    return new Promise((resolve) => {
        let u;
        try { u = new URL(urlStr); } catch (e) { resolve({ ok: false, error: 'URL 非法: ' + e.message }); return; }
        const lib = u.protocol === 'http:' ? require('http') : https;
        const req = lib.request({
            hostname: u.hostname,
            path: u.pathname + u.search,
            method: 'GET',
            headers: headers || {},
            timeout: timeoutMs,
        }, (res) => {
            const chunks = [];
            let n = 0;
            res.on('data', (d) => { n += d.length; if (n <= 16 * 1024 * 1024) chunks.push(d); });
            res.on('end', () => resolve({
                ok: res.statusCode >= 200 && res.statusCode < 300,
                status: res.statusCode,
                buf: Buffer.concat(chunks),
            }));
        });
        req.on('timeout', () => { req.destroy(); resolve({ ok: false, error: '请求超时(' + timeoutMs + 'ms)' }); });
        req.on('error', (e) => resolve({ ok: false, error: String((e && e.message) || e) }));
        req.end();
    });
}

function jget(urlStr, headers, timeoutMs) {
    return httpGetBuffer(urlStr, headers, timeoutMs).then((r) => {
        if (!r.ok) return { err: r.error || ('HTTP ' + r.status) };
        try { return { data: JSON.parse(r.buf.toString('utf8')) }; } catch (e) { return { err: '响应非 JSON' }; }
    });
}

// ===================== 标题归一化（与 tencent-danmaku.cjs 逐字一致） =====================

const _FILLER = ['高清', '1080p', '720p', '480p', '4k', '合集', '全集', '更新', '熟肉', '生肉',
    '国语', '日语', '中字', '双语', '中配', '粤配', 'tv版', '完整版', '未删减', '蓝光', 'bd'];

/**
 * 剥掉「第N集 / 第N话 / EP12 / N」这类**集数后缀**再比相似度。
 *
 * 为什么必须剥：长度比例惩罚（防「进击的巨人」→「无敌巨鲨大战进击巨人」）会把
 * 「狂飙」vs「狂飙第1集」压到 0.57 —— 明明是同一部片，集数后缀让长度比变成 2.4。
 * 集数后缀是平台侧的正常差异（爱奇艺搜索结果是「片名第1集」粒度），不该被罚。
 * 注意只在**尾部**剥，且要求前面还有内容（避免「第1集」整个标题被剥空）。
 */
function stripEpisodeSuffix(s) {
    let t = String(s || '');
    // 形态 2：集号在中间，后面跟本集副标题（「鬼灭之刃柱训练篇第1集 众柱训练会议」）
    // -> 从集号处截断。限定后面必须有空白或直接结束，避免误伤「第一集就结婚」这类片名。
    // 这是实测踩到的坑：只剥尾部会把副标题留下，长度比拉大 -> 0.57 被阈值误拒，
    // 明明爱奇艺有这部番却拿不到弹幕。
    const mid = t.match(/^(.*?)第\s*[\d一二三四五六七八九十]+\s*[集话期回](\s|$)/);
    if (mid && mid[1].trim()) {
        return mid[1].trim();
    }
    // 形态 1：集号在末尾（「狂飙第1集」）
    const patterns = [
        /第\s*\d+\s*[集话期回]$/,
        /第\s*[一二三四五六七八九十]+\s*[集话期回]$/,
        /[Ee][Pp]?\s*\d+$/,
        /\s+\d+$/,
    ];
    for (const re of patterns) {
        const next = t.replace(re, '').trim();
        if (next && next !== t) t = next;
    }
    return t;
}

function normTitle(t) {
    let s = String(t || '').toLowerCase();
    s = s.replace(/<[^>]+>/g, '');
    for (const f of _FILLER) s = s.split(f).join('');
    s = s.replace(/[\s\-_—–·・:：!！?？,，.。'"“”''()（）\[\]【】]/g, '');
    return s;
}

/**
 * 归一化：先剥**集数**后缀（第N集/话/期/EPn/尾部数字），但**保留季号**（第N季），
 * 再套通用清洗。
 *
 * 为什么季号不能剥：季号决定选哪一季的条目，剥了就分不出 S1/S2。
 * 为什么季号又不能被长度惩罚：`庆余年` vs `庆余年 第二季` 长度比 1.57，惩罚 0.85
 * → 0.81 勉强过阈值，但更长的季名会掉下去。所以 titleSim 里对「只差季号后缀」
 * 的情形直接给高分（见下方 seasonOnlyDiff）。
 */
function normTitleForMatch(t) {
    return normTitle(stripEpisodeSuffix(String(t || '').replace(/<[^>]+>/g, '').trim()));
}

/** 剥掉季号后缀（第N季/Season N），用于判断「是否只差季号」。 */
function stripSeasonSuffix(s) {
    return String(s || '').replace(/\s*第\s*[\d一二三四五六七八九十]+\s*[季部期]\s*$/, '')
        .replace(/\s*[Ss]eason\s*\d+\s*$/, '').trim();
}

/**
 * 从标题尾部解析集号（「狂飙第30集」→ 30、「第12话」→ 12、「EP5」→ 5）。
 * 返回 0 = 标题不带集号。
 *
 * 为什么必须解析：爱奇艺搜索结果是**单集粒度**（「狂飙第1集」「狂飙第30集」是同片
 * 的不同条目，各自独立 tvid），不带集号就无法区分。实测不加这步会默认选到
 * 「狂飙第30集」（时长最长被排序顶上去）—— 明明要第 1 集却给了第 30 集的弹幕，
 * 这是比"没弹幕"更糟的错位。
 */
function parseEpisode(t) {
    const s = String(t || '');
    let m = s.match(/第\s*(\d+)\s*[集话期回]/);
    if (m) return parseInt(m[1], 10);
    m = s.match(/第\s*([一二三四五六七八九十]+)\s*[集话期回]/);
    if (m) {
        const cn = m[1];
        if (cn === '十') return 10;
        if (cn[0] === '十') return 10 + (_CN_SEASON[cn.slice(1)] || 0);
        if (cn.slice(-1) === '十') return (_CN_SEASON[cn[0]] || 1) * 10;
        const hm = cn.match(/^(.)十(.)$/);
        if (hm) return (_CN_SEASON[hm[1]] || 1) * 10 + (_CN_SEASON[hm[2]] || 0);
        return _CN_SEASON[cn] || 0;
    }
    m = s.match(/[Ee][Pp]?\s*(\d+)/);
    if (m) return parseInt(m[1], 10);
    return 0;
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

/** 与腾讯源同一份实现：互相包含优先 + **长度比例惩罚**（防「进击的巨人」匹配到
 *  「无敌巨鲨大战进击巨人」这类包含关系误判）+ LCS 兜底。
 *  [lc-2350] 比之前多一步：归一化前**剥集数后缀**（「狂飙第1集」→「狂飙」），
 *  否则长度惩罚会把同片集数后缀误判成不同片（实测 0.57 被阈值拒掉）。 */
function titleSim(a, b) {
    const x = normTitleForMatch(a), y = normTitleForMatch(b);
    if (!x || !y) return 0;
    if (x === y) return 1;
    // 只差季号 → 同类作品的不同季（同一部片的 S1/S2 条目），给高分但不满分，
    // 让调用方的季命中排序去区分（比完长度惩罚更可靠：惩罚会随季名长度漂移）
    const xs = normTitleForMatch(stripSeasonSuffix(String(a || '').replace(/<[^>]+>/g, '')));
    const ys = normTitleForMatch(stripSeasonSuffix(String(b || '').replace(/<[^>]+>/g, '')));
    if (xs && ys && xs === ys) return 0.9;
    const long = Math.max(x.length, y.length);
    const short = Math.min(x.length, y.length);
    const lenRatio = short > 0 ? long / short : Infinity;
    const penalty = lenRatio <= 1.3 ? 1 : (lenRatio <= 1.8 ? 0.85 : (lenRatio <= 2.5 ? 0.6 : 0.35));
    if (x.indexOf(y) >= 0 || y.indexOf(x) >= 0) return 0.95 * penalty;
    return lcsRatio(x, y) * penalty;
}

const ACCEPT_SIM = 0.75;

const _CN_SEASON = { '一': 1, '二': 2, '三': 3, '四': 4, '五': 5, '六': 6, '七': 7, '八': 8, '九': 9, '十': 10 };

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

// ===================== 搜索：番名 → tvId =====================

/**
 * 从爱奇艺搜索结果里挑最合适的一条。
 *
 * 实测坑：搜索结果首位常是**综艺/花絮/预告**条目（`timeLength` 很小或无），
 * 甚至整条 `videoinfos` 为空 —— 必须筛掉，否则会拿到 undefined tvid。
 * 筛选条件：有 tvId、有 itemshortTitle、timeLength > 60（跳过预告片/花絮）。
 */
async function searchTvid(title, seasonNum, epNum) {
    const kw = String(title || '').trim();
    if (!kw) return { ok: false, error: '番名为空' };

    const url = 'https://search.video.iqiyi.com/o?key=' + encodeURIComponent(kw) + '&if=html5';
    const r = await jget(url, IQIYI, 15000);
    if (r.err) return { ok: false, error: '爱奇艺搜索失败: ' + r.err };

    const docinfos = (r.data && r.data.data && r.data.data.docinfos) || [];
    if (!docinfos.length) return { ok: false, error: '爱奇艺无搜索结果' };

    const wantEp = Number(epNum) || 0;
    const cands = [];
    for (const doc of docinfos) {
        const ai = doc && doc.albumDocInfo;
        if (!ai) continue;
        const albumTitle = String(ai.albumTitle || '').replace(/<[^>]+>/g, '');
        const videos = ai.videoinfos || [];
        for (const v of videos) {
            const tvid = v && v.tvId;
            const tl = v && v.timeLength;
            const vt = String((v && v.itemshortTitle) || albumTitle || '').replace(/<[^>]+>/g, '');
            // 跳过预告/花絮/无时长条目（实测「庆余年」首位就是这种，直接 undefined）
            if (!tvid || typeof tl !== 'number' || tl <= 60 || !vt) continue;
            const simV = titleSim(kw, vt);
            const simA = titleSim(kw, albumTitle);
            const sim = simV > simA ? simV : simA;
            const season = parseSeason(vt) || parseSeason(albumTitle) || parseSeason(kw);
            // 该条目自带的集号（爱奇艺搜索结果是单集粒度，一集一个 tvId）
            const ep = parseEpisode(vt);
            cands.push({ tvid: String(tvid), title: vt, albumTitle, duration: tl, sim, season, ep });
        }
    }
    if (!cands.length) {
        return { ok: false, error: '爱奇艺无可用视频（结果里只有预告/花絮）', candidates: docinfos.slice(0, 6).map((d) => String((d.albumDocInfo && d.albumDocInfo.albumTitle) || '').replace(/<[^>]+>/g, '')) };
    }

    // 排序优先级（顺序即重要性）：
    //   1. 季命中（请求季 > 0 时，标了该季的排前）
    //   2. **集号命中**（请求集 > 0 时，该集排前）—— 不加这条会默认选时长最长的第 30 集
    //   3. 相似度
    //   4. 时长（同一集有多个版本时取长的那个）
    const epRank = (c) => {
        if (wantEp <= 0) return 1;          // 未指定集数：不参与排序
        if (c.ep === wantEp) return 0;      // 精确命中
        if (c.ep === 0) return 1;           // 条目无集号（整季/电影）
        return 2;                           // 明确是别的集
    };
    const seasonRank = (c) => {
        if (seasonNum <= 0) return 1;
        if (c.season === seasonNum) return 0;
        if (c.season === 0) return 1;
        return 2;
    };
    cands.sort((a, b) => {
        const sr = seasonRank(a) - seasonRank(b);
        if (sr !== 0) return sr;
        const er = epRank(a) - epRank(b);
        if (er !== 0) return er;
        if (b.sim !== a.sim) return b.sim - a.sim;
        return b.duration - a.duration;
    });

    const best = cands[0];
    if (best.sim < ACCEPT_SIM) {
        log(`最优候选「${best.title}」sim=${best.sim.toFixed(2)} < 阈值${ACCEPT_SIM} → 不采用`);
        return {
            ok: false,
            error: `爱奇艺《${best.title}》相似度仅 ${best.sim.toFixed(2)}（阈值 ${ACCEPT_SIM}），疑似非同片，不采用`,
            candidates: cands.slice(0, 6).map((c) => `${c.title} sim=${c.sim.toFixed(2)}`),
        };
    }
    // 集号不符时如实告警（不静默：用户会看到「点第1集却来了第30集的弹幕」）
    let epMismatch = false;
    if (wantEp > 0 && best.ep > 0 && best.ep !== wantEp) {
        epMismatch = true;
        log(`⚠️ 集号不符：要第${wantEp}集，命中「${best.title}」（第${best.ep}集）`);
    }
    log(`爱奇艺命中「${best.title}」sim=${best.sim.toFixed(2)} season=${best.season} ep=${best.ep} tvid=${best.tvid} (${best.duration}s)`);
    return {
        ok: true,
        tvid: best.tvid,
        title: best.title,
        albumTitle: best.albumTitle,
        duration: best.duration,
        sim: best.sim,
        season: best.season,
        ep: best.ep,
        ep_mismatch: epMismatch,
        candidates: cands.slice(0, 6).map((c) => `${c.title}${c.ep ? '(第' + c.ep + '集)' : ''} sim=${c.sim.toFixed(2)}`),
    };
}

// ===================== 弹幕拉取 =====================

/**
 * tvid → 目录分片路径。爱奇艺的规则：前补 "0000"，取倒数第 4~3 位作一级、
 * 倒数第 2 位起作二级。（实测 tvid=3493131456125200 → 52/00；
 * tvid=399163400 → 34/00，两者都验证通过）
 */
function tvidPath(tvid) {
    const t = '0000' + String(tvid);
    return { s1: t.slice(-4, -2), s2: t.slice(-2) };
}

/** 解析一段的 XML → [{t, text, mode, color}]。position 映射见文件头。 */
function parseXml(xml) {
    const out = [];
    const re = /<bulletInfo>([\s\S]*?)<\/bulletInfo>/g;
    let m;
    const pick = (body, tag) => {
        const mm = body.match(new RegExp('<' + tag + '>([\\s\\S]*?)</' + tag + '>'));
        return mm ? mm[1] : '';
    };
    while ((m = re.exec(xml)) !== null) {
        const body = m[1];
        const text = pick(body, 'content').trim();
        if (!text) continue;
        const st = parseFloat(pick(body, 'showTime'));
        if (!isFinite(st)) continue;
        const pos = parseInt(pick(body, 'position') || '0', 10) || 0;
        // 100/108 = 高级弹幕（变色/移动特效）：ASS 渲染不支持，硬套会错位 → 丢弃
        if (pos === 100 || pos === 108) continue;
        // position → B站 mode（parse.lua 的 MODE_BLOCK_TAG：1=scroll 4=bottom 5=top）
        let mode = 1;
        if (pos === 1) mode = 5;        // 顶部
        else if (pos === 200) mode = 4; // 底部
        // color 是 hex 字符串，大小写混杂，统一按 6 位大写解析
        const hex = (pick(body, 'color') || 'ffffff').trim().replace(/^#/, '').toUpperCase();
        const color = /^[0-9A-F]{6}$/.test(hex) ? parseInt(hex, 16) : 16777215;
        out.push({ t: st, text: text, mode: mode, color: color });
    }
    return out;
}

/** 拉一段（段号从 1 开始）。返回 [] = 无此段（末尾）。 */
async function fetchSegment(tvid, segIdx) {
    const { s1, s2 } = tvidPath(tvid);
    const url = `https://cmts.iqiyi.com/bullet/${s1}/${s2}/${tvid}_300_${segIdx}.z`;
    const r = await httpGetBuffer(url, IQIYI, 15000);
    if (!r.ok || !r.buf || !r.buf.length) return [];
    let xml;
    try {
        xml = zlib.inflateSync(r.buf).toString('utf8');
    } catch (e) {
        // 有的段是明文 JSON 错误体（{"code":"NoSuchKey"}），不是 zlib
        log(`段${segIdx} 解压失败（可能是末尾/错误体）: ${r.buf.slice(0, 60).toString('utf8')}`);
        return [];
    }
    if (xml.indexOf('<bulletInfo>') < 0) return [];
    return parseXml(xml);
}

/**
 * 拉整集。按段序递增，连续 2 段空即停（末段特征；单段偶发空可能是抖动，给一次容错）。
 * @returns {{ok:boolean, list?:Array, segments?:number, error?:string}}
 */
async function fetchAll(tvid, maxSeg) {
    if (!tvid) return { ok: false, error: 'tvid 为空' };
    const limit = Math.min(maxSeg || MAX_SEG, MAX_SEG);
    const all = [];
    let emptyStreak = 0;
    let seg = 0;
    for (let i = 1; i <= limit; i++) {
        const part = await fetchSegment(tvid, i);
        seg = i;
        if (!part.length) {
            emptyStreak++;
            if (emptyStreak >= 2) break;
        } else {
            emptyStreak = 0;
            for (const d of part) all.push(d);
        }
    }
    if (!all.length) return { ok: false, error: '爱奇艺未返回任何弹幕（该视频可能无弹幕池）', segments: seg };
    log(`tvid=${tvid} 拉到 ${all.length} 条，扫了 ${seg} 段`);
    return { ok: true, list: all, segments: seg };
}

// ===================== 输出 XML（与 tencent-danmaku.cjs / bili_danmaku.js 同格式） =====================

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
 * 端到端：番名(+季+集) → tvid → 弹幕 → 写 XML。
 * @param {string} title 番名
 * @param {number} season 季号（0 = 不筛）
 * @param {string} out 输出 XML 路径
 * @param {number} timeoutMs 全局超时
 * @param {string} tvidHint 已知 tvid 时直接用它（跳过搜索）
 * @param {number} epNum 集号（0 = 不筛）。必传：爱奇艺搜索是单集粒度，
 *        不传会选到「时长最长」的那一集（实测默认命中「狂飙第30集」）。
 */
async function run(title, season, out, timeoutMs, tvidHint, epNum) {
    const started = Date.now();
    const budget = timeoutMs || 90000;
    const remain = () => Math.max(1000, budget - (Date.now() - started));

    let tvid = tvidHint ? String(tvidHint).trim() : '';
    let matchedTitle = '';
    let sim = null;
    let seasonHit = false;
    let epMismatch = false;
    let source = 'iqiyi';
    let candidates = null;
    let duration = null;
    const wantEp = Number(epNum) || 0;

    if (!tvid) {
        const found = await Promise.race([
            searchTvid(title, Number(season) || 0, wantEp),
            new Promise((r) => setTimeout(() => r({ ok: false, error: '爱奇艺搜索超时' }), Math.min(20000, remain()))),
        ]);
        if (!found.ok) {
            return {
                ok: false, error: found.error || '未找到爱奇艺视频',
                source: source, candidates: found.candidates || null, video_found: false,
            };
        }
        tvid = found.tvid;
        matchedTitle = found.title || '';
        sim = typeof found.sim === 'number' ? found.sim : null;
        seasonHit = Number(season) > 0 && found.season === Number(season);
        epMismatch = !!found.ep_mismatch;
        candidates = found.candidates || null;
        duration = found.duration || null;
    } else {
        matchedTitle = title || '';
        source = 'iqiyi-manual';
    }

    const res = await Promise.race([
        fetchAll(tvid, null),
        new Promise((r) => setTimeout(() => r({ ok: false, error: '弹幕拉取超时(' + budget + 'ms)' }), remain())),
    ]);
    if (!res.ok) {
        return {
            ok: false, error: res.error || '爱奇艺弹幕为空',
            source: source, tvid: tvid, matched_title: matchedTitle, sim: sim,
            video_found: true, candidates: candidates,
        };
    }

    const count = writeXml(out, res.list);
    log(`✅ 爱奇艺弹幕 ${count} 条 -> ${out} (tvid=${tvid} ${matchedTitle})`);
    return {
        ok: true,
        tvid: tvid,
        title: title,
        matched_title: matchedTitle || title,
        sim: sim,
        danmaku_count: count,
        source: source,
        season_match: seasonHit,
        ep_mismatch: epMismatch,
        video_found: true,
        candidates: candidates,
        duration: duration,
        segments: res.segments || null,
    };
}

/** 仅搜索（供手动候选列表展示），不拉弹幕。 */
async function searchCandidates(title, season, epNum) {
    const found = await searchTvid(title, Number(season) || 0, Number(epNum) || 0);
    if (!found.ok) return { ok: false, error: found.error, candidates: found.candidates };
    return {
        ok: true,
        tvid: found.tvid,
        matched_title: found.title,
        album_title: found.albumTitle,
        sim: found.sim,
        season: found.season,
        ep: found.ep,
        ep_mismatch: !!found.ep_mismatch,
        duration: found.duration,
        candidates: found.candidates,
    };
}

module.exports = {
    run: run,
    search_candidates: searchCandidates,
    search_tvid: searchTvid,
    tvid_path: tvidPath,
    fetch_all: fetchAll,
    parse_xml: parseXml,
    write_xml: writeXml,
    setLogSink: setLogSink,
    _internal: { titleSim: titleSim, parseSeason: parseSeason, parseEpisode: parseEpisode, normTitle: normTitle, stripEpisodeSuffix: stripEpisodeSuffix },
};
