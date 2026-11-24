#!/usr/bin/env node
/**
 * [lc-2337] 内置苹果CMS 采集源生成器。
 *
 * 为什么需要它：TVBox 是 Android 壳，自带 Dalvik 能跑 csp/jar/drpy 插件源；AniBox 没有
 * Java 运行时，只能消费**明文苹果CMS 接口**（`api.php/provide/vod`）。所以内置源的做法是
 * 「本地把这些公开 TVBox 配置拆好、实测筛活，再随包分发」，而不是运行时去抓。
 *
 * 用法：
 *   node tools/harvest-maccms-sources.mjs              # 只跑：采集 + 筛活 + 打印结果
 *   node tools/harvest-maccms-sources.mjs --write      # 通过筛活的源写回 builtin_maccms.json
 *   node tools/harvest-maccms-sources.mjs --urls a,b   # 只跑指定配置链接
 *
 * 筛活口径（全部通过才算可用，任一不过即丢弃）：
 *   1. 三个关键词搜索都返回非空 list（只有单个关键词命中通常是站点数据稀疏，不是坏源）；
 *   2. `ac=detail&ids=` 详情返回可解析的 vod_play_url；
 *   3. 首集能解析出媒体地址（直链或可嗅探页面）；
 *   4. 标题不含成人内容（本项目不做这类源）；
 *   5. 同接口地址去重（不同名字同一 api 在 TVBox 配置里很常见）。
 *
 * 抓取契约（实测）：部分站按 UA 分流 —— 浏览器 UA 给网页，`okhttp/*` 给图片，真正的配置
 * base64 拼在图片字节流尾部。配置 JSON 允许 `//` 行注释与尾逗号（CatVod 宽容语法）。
 */
import https from 'node:https';
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const OUT_JSON = path.join(HERE, '..', 'src-tauri', 'src', 'builtin_maccms.json');

/** 默认配置清单：qist/tvbox 与 gaotianliuyun/gao 仓库的根配置（这两个是 TVBox 圈维护最勤的）。 */
const DEFAULT_URLS = [
  'https://raw.githubusercontent.com/qist/tvbox/master/0821.json',
  'https://raw.githubusercontent.com/qist/tvbox/master/0825.json',
  'https://raw.githubusercontent.com/qist/tvbox/master/0826.json',
  'https://raw.githubusercontent.com/qist/tvbox/master/0827.json',
  'https://raw.githubusercontent.com/qist/tvbox/master/9918.json',
  'https://raw.githubusercontent.com/qist/tvbox/master/99188.json',
  'https://raw.githubusercontent.com/qist/tvbox/master/dianshi.json',
  'https://raw.githubusercontent.com/qist/tvbox/master/jsm.json',
  'https://raw.githubusercontent.com/qist/tvbox/master/js.json',
  'https://raw.githubusercontent.com/qist/tvbox/master/fty.json',
  'https://raw.githubusercontent.com/qist/tvbox/master/367.json',
  'https://raw.githubusercontent.com/qist/tvbox/master/XYQ.json',
  'https://raw.githubusercontent.com/gaotianliuyun/gao/master/0821.json',
  'https://raw.githubusercontent.com/gaotianliuyun/gao/master/0825.json',
  'https://raw.githubusercontent.com/gaotianliuyun/gao/master/0827.json',
  'https://raw.githubusercontent.com/gaotianliuyun/gao/master/XYQ.json',
];

const CONFIG_UA = 'okhttp/3.12.0';   // 配置站按 UA 分流，必须用这个才拿到图片载荷
const API_UA = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/131.0 Safari/537.36';
const KEYWORDS = ['庆余年', '流浪地球', '三体'];
const ADULT = /成人|色情|情色|福利|里番|约炮|裸|性爱|做爱|自慰|高潮|淫|18禁|无码|有码/i;

// ───────────────────────── HTTP ─────────────────────────

function request(url, ua, { timeout = 15000, redirects = 5 } = {}) {
  return new Promise((resolve) => {
    let mod;
    try {
      mod = new URL(url).protocol === 'https:' ? https : http;
    } catch {
      return resolve({ status: 0, headers: {}, body: Buffer.alloc(0) });
    }
    const req = mod.get(url, { headers: { 'User-Agent': ua, Accept: '*/*' }, timeout, rejectUnauthorized: false }, (res) => {
      if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location && redirects > 0) {
        res.resume();
        let next;
        try {
          next = new URL(res.headers.location, url).toString();
        } catch {
          return resolve({ status: res.statusCode, headers: res.headers, body: Buffer.alloc(0) });
        }
        return resolve(request(next, ua, { timeout, redirects: redirects - 1 }));
      }
      const chunks = [];
      res.on('data', (c) => chunks.push(c));
      res.on('end', () => resolve({ status: res.statusCode, headers: res.headers, body: Buffer.concat(chunks) }));
    });
    req.on('timeout', () => {
      req.destroy();
      resolve({ status: 0, headers: {}, body: Buffer.alloc(0) });
    });
    req.on('error', () => resolve({ status: 0, headers: {}, body: Buffer.alloc(0) }));
  });
}

const getText = async (url) => {
  const r = await request(url, API_UA);
  return { status: r.status, text: r.body.toString('utf8') };
};

// ───────────────────────── 配置解包（与 src-tauri/src/tvbox.rs 同口径）─────────────────────────

const isB64Char = (c) => /[A-Za-z0-9+/=\r\n]/.test(String.fromCharCode(c));

function base64Runs(buf, min) {
  const out = [];
  let cur = [];
  for (const c of buf) {
    if (isB64Char(c)) cur.push(c);
    else {
      if (cur.filter((x) => x !== 13 && x !== 10).length >= min) out.push(Buffer.from(cur));
      cur = [];
    }
  }
  if (cur.filter((x) => x !== 13 && x !== 10).length >= min) out.push(Buffer.from(cur));
  return out;
}

/** 明文 JSON 直接用；否则从尾部往前试解 base64 块，第一个解出来是 JSON 对象的采纳。 */
export function extractConfigText(buf) {
  const asText = buf.toString('utf8');
  if (asText.trimStart().replace(/^\uFEFF/, '').trimStart().startsWith('{')) return asText;
  const runs = base64Runs(buf, 64);
  for (let i = runs.length - 1; i >= 0; i--) {
    const cleaned = runs[i].toString('latin1').replace(/[\r\n \t]/g, '');
    let decoded;
    try {
      decoded = Buffer.from(cleaned, 'base64').toString('utf8');
    } catch {
      continue;
    }
    if (decoded.trimStart().startsWith('{')) return decoded;
  }
  return null;
}

/** 去 `//` 行注释（不碰字符串内）与尾逗号。 */
export function stripJsonComments(raw) {
  const BS = String.fromCharCode(92);
  let out = '';
  let inStr = false;
  let esc = false;
  for (let i = 0; i < raw.length; i++) {
    const c = raw[i];
    if (inStr) {
      out += c;
      if (esc) esc = false;
      else if (c === BS) esc = true;
      else if (c === '"') inStr = false;
      continue;
    }
    if (c === '"') {
      inStr = true;
      out += c;
      continue;
    }
    if (c === '/' && raw[i + 1] === '/') {
      while (i < raw.length && raw[i] !== '\n') i++;
      continue;
    }
    out += c;
  }
  return out.replace(/,(\s*[}\]])/g, '$1');
}

/** 苹果CMS V10 标准接口判定：路径含 api.php 且 `/provide/vod` 后是边界。 */
export function isMaccmsApi(api) {
  if (typeof api !== 'string') return false;
  const low = api.trim().toLowerCase();
  if (!/^https?:\/\//.test(low)) return false;
  const p = low.split(/[?#]/)[0];
  if (!p.includes('api.php')) return false;
  const i = p.indexOf('/provide/vod');
  if (i < 0) return false;
  const after = p.slice(i + '/provide/vod'.length);
  return after === '' || after.startsWith('/');
}

const cleanName = (raw) =>
  String(raw || '')
    .replace(/[\u{1F000}-\u{1FAFF}\u{2600}-\u{27BF}\u{FE00}-\u{FE0F}]/gu, '')
    .replace(/[┃｜|]/g, '')
    .trim() || String(raw || '').trim();

const hostOf = (u) => {
  try {
    return new URL(u).host;
  } catch {
    return 'unknown';
  }
};

/** 从一份配置里抽所有明文苹果CMS 采集源（同接口去重）。 */
export function extractMaccms(cfg) {
  const out = [];
  const seen = new Set();
  for (const s of cfg.sites || []) {
    let api = typeof s.api === 'string' ? s.api.trim() : '';
    if (!isMaccmsApi(api) && typeof s.ext === 'string' && isMaccmsApi(s.ext.trim())) api = s.ext.trim();
    if (!isMaccmsApi(api)) continue;
    const key = api.toLowerCase().replace(/\/+$/, '');
    if (seen.has(key)) continue;
    seen.add(key);
    out.push({
      name: cleanName(s.name) || hostOf(api),
      api,
      referer: (s.ext && typeof s.ext === 'object' && s.ext.siteUrl) || '',
    });
  }
  return out;
}

// ───────────────────────── 筛活 ─────────────────────────

/** 规范请求 URL：剥掉 /at/xml 这类渲染后缀，替换/补齐 ac 参数。 */
function apiUrl(api, params) {
  let base = api.trim().replace(/\/at\/[a-z]+\/?$/i, '/');
  base = base.replace(/([?&])(ac|wd|ids)=[^&]*/g, '$1').replace(/[?&]$/, '');
  const sep = base.includes('?') ? '&' : '?';
  const qs = Object.entries(params)
    .map(([k, v]) => `${k}=${encodeURIComponent(v)}`)
    .join('&');
  return `${base}${sep}${qs}`.replace(/&&+/g, '&');
}

/**
 * 带重试的取文本（筛活阶段用）。
 *
 * 为什么必须重试：这些采集站经常「第一次连接被重置、第二次正常」；不重试会把
 * 一批**其实可用**的源误判成死源（本轮就踩过：`0821.json` 单次 HTTP 0 导致整份
 * 配置的 7 条候选全丢，其中 ffzy5/360zy/apibdzy 实际都活着）。只对网络层失败
 * （status 0）重试，HTTP 4xx/5xx 是站点明确表态，不重试。
 */
async function getTextRetry(url, tries = 3) {
  let last = { status: 0, text: '' };
  for (let i = 0; i < tries; i++) {
    last = await getText(url);
    if (last.status !== 0) return last;
    await new Promise((r) => setTimeout(r, 600 * (i + 1)));
  }
  return last;
}

/** 抓配置也带重试：raw.githubusercontent.com 偶发连接重置。 */
async function fetchConfig(url, tries = 3) {
  let last = null;
  for (let i = 0; i < tries; i++) {
    const r = await request(url, CONFIG_UA, { timeout: 25000 });
    if (r.status >= 200 && r.status < 300) return r;
    last = r;
    // HTTP 404 之类重试无意义，直接放弃
    if (r.status >= 400) return r;
    await new Promise((res) => setTimeout(res, 600 * (i + 1)));
  }
  return last;
}

async function probeSource(cand) {
  const r = { ...cand, searches: {}, detail: null, ok: false, reason: '' };

  for (const kw of KEYWORDS) {
    const { status, text } = await getTextRetry(apiUrl(cand.api, { ac: 'videolist', wd: kw }));
    if (status !== 200) {
      r.searches[kw] = `HTTP ${status}`;
      r.reason ||= `搜索 HTTP ${status}`;
      return r;
    }
    let j;
    try {
      j = JSON.parse(text);
    } catch {
      r.searches[kw] = '非JSON';
      r.reason ||= '搜索返回非 JSON';
      return r;
    }
    const list = j.list || j.data || [];
    const n = Array.isArray(list) ? list.length : 0;
    r.searches[kw] = n;
    if (n === 0) {
      r.reason ||= `关键词「${kw}」无结果`;
      return r;
    }
    if (ADULT.test(String(list[0]?.vod_name || '')) || ADULT.test(cand.name)) {
      r.reason = '成人内容';
      return r;
    }
  }

  // 详情 / 选集链路：用最后一个关键词的结果取 vod_id
  const sres = await getTextRetry(apiUrl(cand.api, { ac: 'videolist', wd: KEYWORDS[KEYWORDS.length - 1] }));
  let vodId = '';
  try {
    vodId = String((JSON.parse(sres.text).list || [])[0]?.vod_id || '');
  } catch {}
  if (!vodId) {
    r.reason = '取不到 vod_id';
    return r;
  }
  const dres = await getTextRetry(apiUrl(cand.api, { ac: 'detail', ids: vodId }));
  if (dres.status !== 200) {
    r.reason = `详情 HTTP ${dres.status}`;
    return r;
  }
  let dj;
  try {
    dj = JSON.parse(dres.text);
  } catch {
    r.reason = '详情非 JSON';
    return r;
  }
  const dl = (dj.list || [])[0];
  const playUrl = String(dl?.vod_play_url || '');
  if (!playUrl.includes('$')) {
    r.reason = '详情无播放地址';
    return r;
  }
  const roads = playUrl.split('$$$');
  const firstEp = (roads[0] || '').split('#')[0] || '';
  const epUrl = firstEp.split('$').slice(1).join('$');
  r.detail = {
    roads: roads.length,
    episodes: roads[0] ? roads[0].split('#').length : 0,
    playFrom: String(dl?.vod_play_from || '').split('$$$'),
    direct: /\.(m3u8|mp4)(\?|$)/i.test(epUrl),
    sample: epUrl.slice(0, 100),
  };
  r.ok = true;
  return r;
}

// ───────────────────────── 生成规则 JSON ─────────────────────────

function ruleOf(name, api) {
  const base = new URL(api).origin;
  return {
    name,
    version: '1.0.0',
    baseURL: base,
    searchMode: 'api',
    searchApiConfig: {
      request: { method: 'GET', url: api, query: { ac: 'videolist', wd: '@keyword' } },
      listPath: '$.list[*]',
      namePath: '$.vod_name',
      sourcePath: '$.vod_id',
    },
    chapterMode: 'api',
    chapterApiConfig: {
      request: { method: 'GET', url: api, query: { ac: 'detail', ids: '@source' } },
      format: 'delimited',
      roadNamesPath: '$.list[*].vod_play_from',
      roadEpisodesPath: '$.list[*].vod_play_url',
    },
  };
}

// ───────────────────────── main ─────────────────────────

async function main() {
  const argv = process.argv.slice(2);
  const write = argv.includes('--write');
  const urlsArg = argv.find((a) => a.startsWith('--urls='));
  const urls = urlsArg ? urlsArg.slice('--urls='.length).split(',') : DEFAULT_URLS;

  console.log(`采集 ${urls.length} 个 TVBox 配置…`);
  const cands = new Map();
  for (const url of urls) {
    const r = await fetchConfig(url);
    if (r.status < 200 || r.status >= 300) {
      console.log(`  [HTTP] ${url} — ${r.status}`);
      continue;
    }
    const text = extractConfigText(r.body);
    if (!text) {
      console.log(`  [SKIP] ${url} — 无配置`);
      continue;
    }
    let cfg;
    try {
      cfg = JSON.parse(stripJsonComments(text));
    } catch (e) {
      console.log(`  [FAIL] ${url} — ${e.message.slice(0, 50)}`);
      continue;
    }
    const found = extractMaccms(cfg);
    console.log(`  [OK  ] ${url} — sites=${(cfg.sites || []).length} 苹果CMS=${found.length}`);
    for (const m of found) {
      const k = m.api.toLowerCase().replace(/\/+$/, '');
      if (!cands.has(k)) cands.set(k, m);
    }
  }

  const list = [...cands.values()];
  console.log(`\n候选 ${list.length} 条，开始筛活（${KEYWORDS.join('/')}）…`);
  const results = [];
  for (const c of list) {
    const r = await probeSource(c);
    results.push(r);
    const tag = r.ok ? '[OK  ]' : '[FAIL]';
    const info = r.ok
      ? `线路${r.detail.roads} 集${r.detail.episodes} ${r.detail.direct ? '直链' : '需嗅探'}`
      : r.reason;
    console.log(`  ${tag} ${String(c.name).padEnd(18)} ${info}`);
  }

  const ok = results.filter((r) => r.ok);
  console.log(`\n━━━ 全链路可用 ${ok.length}/${results.length} ━━━`);
  ok.forEach((r) =>
    console.log(`  ${r.name}\t${r.api}\t线路${r.detail.roads}\t集${r.detail.episodes}\t${r.detail.direct ? '直链' : '需嗅探'}`),
  );

  if (!write) {
    console.log('\n（未写文件。加 --write 才更新 builtin_maccms.json）');
    return;
  }
  if (ok.length === 0) {
    console.error('\n没有可用源，拒绝写入（避免把内置源清空）');
    process.exit(1);
  }
  const rules = ok.map((r) => ruleOf(r.name, r.api));
  fs.writeFileSync(OUT_JSON, JSON.stringify(rules, null, 2) + '\n');
  console.log(`\n→ 已写入 ${OUT_JSON}（${rules.length} 条）`);
}

main();
