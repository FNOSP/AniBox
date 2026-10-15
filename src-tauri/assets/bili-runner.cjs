#!/usr/bin/env node
/**
 * [lc-1561] 弹幕源编排器 —— **由主仓库 biliRunner.ts 原样转译**（tsc CommonJS 产物），
 * 业务逻辑一字未改：自建源 → 内置 B站 → 弹弹play 的优先级链、低于聚合阈值时叠加合并、
 * 逐源 traces、候选三源合并与序号重排、伪 id 直取分支 —— 全部保留。
 *
 * 两处环境替换（见下方注释）：
 *   ① electron → ./danmu-api-deps.cjs 桩（脚本目录由 shim 传入）
 *   ② dandanplay → 「由 MPV 弹幕脚本兜底」替身：该模块内嵌**第三方服务凭证密文常量**，
 *      按本仓库入库规则（凭据不入库）不移植；MPV 链路的弹弹play 兜底本就由 Lua 侧
 *      apis/dandanplay.lua 承担（内置凭证降级：暂缓 /comment → 其他源都失败才发），
 *      用户可见结果一致（实测日志：「自动补源：B站弹幕下载失败」→ Lua 立即 flush /comment）。
 */
'use strict';
"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || (function () {
    var ownKeys = function(o) {
        ownKeys = Object.getOwnPropertyNames || function (o) {
            var ar = [];
            for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
            return ar;
        };
        return ownKeys(o);
    };
    return function (mod) {
        if (mod && mod.__esModule) return mod;
        var result = {};
        if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
        __setModuleDefault(result, mod);
        return result;
    };
})();
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.resolveDanmakuScriptDir = resolveDanmakuScriptDir;
exports.runBiliDanmaku = runBiliDanmaku;
exports.resetBiliModule = resetBiliModule;
exports.clearBiliSourceLock = clearBiliSourceLock;
exports.checkBiliCookie = checkBiliCookie;
exports.runBiliDanmakuCandidates = runBiliDanmakuCandidates;
exports.runBiliDanmakuByBvid = runBiliDanmakuByBvid;
exports.listBiliDanmakuPages = listBiliDanmakuPages;
const fs = __importStar(require("fs"));
const path = __importStar(require("path"));
const { app: electron_1_app, setScriptDir } = require("./danmu-api-deps.cjs");
const electron_1 = { app: electron_1_app };
const logger_1 = { default: require("./danmu-api-deps.cjs").logger };
const danmuApi = __importStar(require("./danmu-api.cjs"));
const dandanplay = require("./danmu-api-deps.cjs").dandanplayLuaOwned;
const log = logger_1.default.component('biliRunner');
let cachedModule = null;
let logSinkBound = false;
// ---- 候选 uosc_danmaku 脚本目录（与 biliCookie.ts / biliDanmaku.ts 保持一致）----
// [lc-1288] 导出：每日 Cookie 体检的状态文件也要写进这个目录（供 MPV Lua 菜单同步读取）
function resolveDanmakuScriptDir() {
    // [lc-1561] shim 启动时就确知脚本目录（由 mpv 路径推导后经 argv 传入，且已校验
    // bili_danmaku.js 存在）。主仓库那套 electron 路径探测在 Tauri 下没有意义，直接返回它。
    return require("./danmu-api-deps.cjs").getScriptDir();
}

function loadModule() {
    if (cachedModule)
        return cachedModule;
    const dir = resolveDanmakuScriptDir();
    if (!dir) {
        throw new Error('未找到 uosc_danmaku 脚本目录，无法加载 bili_danmaku.js');
    }
    const jsPath = path.join(dir, 'bili_danmaku.js');
    if (!fs.existsSync(jsPath)) {
        throw new Error('bili_danmaku.js 不存在: ' + jsPath);
    }
    // eslint-disable-next-line @typescript-eslint/no-var-requires
    const mod = require(jsPath);
    if (!logSinkBound && typeof mod.setLogSink === 'function') {
        // 把弹幕脚本内部日志转发到主进程 logger（进 app.log），便于排查
        mod.setLogSink((line) => log.info('[bili_danmaku] ' + line));
        logSinkBound = true;
    }
    cachedModule = mod;
    return mod;
}
// ===================== [lc-1288] 弹幕 XML 合并工具 =====================
// 背景：自建源命中但条数低于聚合阈值时，需要叠加内置 B站 的弹幕。两条链路各自把
// bilibili 格式的 XML 写到磁盘（danmuApi 写原始 <i> 根，bili_danmaku.js 写 <danmaku> 根），
// 消费方（Lua / 网页解析）只认 <d p="...">text</d> 条目，因此按条目级抽行合并即可，
// 统一重写为 <danmaku> 根（与 _write_xml 输出一致）。
const DM_LINE_RE = /<d\s[^>]*p="[^"]*"[^>]*>[\s\S]*?<\/d>/g;
/** 读取弹幕 XML 里的全部 <d ...>...</d> 条目行（文件不存在/解析为空返回 []）。 */
function readDanmakuLines(file) {
    try {
        if (!fs.existsSync(file))
            return [];
        const body = fs.readFileSync(file, 'utf8');
        const lines = body.match(DM_LINE_RE) || [];
        return lines.map((l) => l.trim()).filter(Boolean);
    }
    catch (e) {
        log.warn('[biliRunner] 读取弹幕 XML 失败: ' + (e?.message || e) + ' | ' + file);
        return [];
    }
}
/** 以 <danmaku> 根重写弹幕 XML（条目行去重：同 p 同文本视为同一条）。 */
function writeMergedDanmakuXml(out, lines) {
    const seen = new Set();
    const uniq = [];
    for (const l of lines) {
        if (seen.has(l))
            continue;
        seen.add(l);
        uniq.push(l);
    }
    const dir = path.dirname(out);
    if (dir && !fs.existsSync(dir))
        fs.mkdirSync(dir, { recursive: true });
    const body = ['<?xml version="1.0" encoding="UTF-8"?>', '<danmaku>', ...uniq, '</danmaku>'].join('\n');
    fs.writeFileSync(out, body + '\n', 'utf8');
    return uniq.length;
}
/**
 * 在主进程内运行 bili_danmaku.js，获取 B站弹幕并写出 XML 到 out。
 * @param title   干净番名
 * @param ep      集数（0=仅标题搜索）
 * @param out     输出 XML 路径（run 内部会确保父目录存在）
 * @param threshold 聚合阈值（可选，默认 1500）
 * @param season  季数（可选，0/undefined=不启用季过滤；>0 时优先精确匹配该季，根治跨季错配）
 * @param timeoutMs 超时保护（默认 60000ms），超时返回 {ok:false}
 * @param epTitle [lc-1220] 播放侧本集标题（供自建源核验未标季条目的分集归属；空串=无核验材料）
 * @returns 结果对象（ok=true 表示成功并写出 XML）
 */
/**
 * 内置 B站 链路：带超时保护地运行 bili_danmaku.js run()。
 * [lc-1288] 从 runBiliDanmaku 主体抽出，供「首次获取」与「自建源低于阈值叠加合并」共用。
 */
async function runBiliChain(title, ep, out, threshold, season, timeoutMs = 60000, altTitle = '') {
    let mod;
    try {
        mod = loadModule();
    }
    catch (e) {
        log.warn('[biliRunner] 加载 bili_danmaku.js 失败: ' + (e?.message || e));
        return { ok: false, error: '弹幕脚本加载失败: ' + (e?.message || e) };
    }
    try {
        // [lc-1265] altTitle 一路透传进 bili_danmaku.js run()：脏番名官方区 0 命中时，
        // 脚本内用「整串→番名首段→alt」换词重试（石纪元S2 实机：fnOS 原标题此前在这被丢弃）。
        const runP = Promise.resolve(mod.run(title, ep, out, threshold, season, altTitle || ''));
        let timeoutHandle = null;
        const timeoutP = new Promise((resolve) => {
            timeoutHandle = setTimeout(() => resolve({ ok: false, error: `弹幕获取超时(${timeoutMs}ms)` }), timeoutMs);
        });
        const r = await Promise.race([runP, timeoutP]);
        if (timeoutHandle)
            clearTimeout(timeoutHandle);
        return (r && typeof r === 'object') ? r : { ok: false, error: '未知错误（run 无返回）' };
    }
    catch (e) {
        log.warn('[biliRunner] run 异常: ' + (e?.message || e));
        return { ok: false, error: String(e?.message || e) };
    }
}
async function runBiliDanmaku(title, ep, out, threshold, season, timeoutMs = 60000, allowBiliFallback = true, epTitle = '', seriesKey = '', altTitle = '') {
    // [lc-1226] 逐源记录本次尝试结果，随结果回传给两处「弹幕详情」面板，
    // 让用户看到三个来源各自是被跳过、试过没中、还是提供了最终弹幕。
    const traces = [];
    // [lc-1101] 自建弹幕接口（danmu_api）优选：命中即返回，未命中(null)原样降级到下面的内置 B站 链路。
    //   放在 loadModule() 之前，命中时连 bili_danmaku.js 都不必加载。
    const danmuApiId = danmuApi.selfHostedSourceLabel();
    const apiActive = danmuApi.isActive();
    if (!apiActive) {
        traces.push({
            id: 'danmu_api', attempted: false, used: false,
            skippedReason: danmuApi.isEnabledButInvalid()
                ? '已开启但服务地址为空/非法（到「弹幕设置 → 自建弹幕接口」填写地址）'
                : '未启用（到「弹幕设置 → 自建弹幕接口」开启，或从内置 B站 获取）',
        });
    }
    const pre = await danmuApi.autoFetch(String(title || ''), Number(ep) || 0, out, Number(season) || 0, epTitle, seriesKey, altTitle);
    if (pre) {
        // [lc-1288] 自建源命中但条数低于聚合阈值 → 不再短路，继续跑内置 B站 并按条目合并。
        // 旧逻辑「命中即用」让 renren 这类只有个位数弹幕的条目直接终结整条链路，
        // B站 上两万条弹幕的官方正片永远轮不到（用户实测：自建源仅 1 条也直接用）。
        const th = Number(threshold) || 0;
        const selfCount = Number(pre.danmaku_count) || 0;
        if (th > 0 && selfCount < th && pre.ok) {
            const selfLines = readDanmakuLines(out);
            if (selfLines.length > 0) {
                // B站 写临时文件：失败时自建源 XML（out）原样保留，成功时合并回写 out
                const tmpOut = out + '.bili.tmp.xml';
                const biliRes = await runBiliChain(title, ep, tmpOut, threshold, season, timeoutMs, altTitle);
                if (biliRes && biliRes.ok) {
                    const biliLines = readDanmakuLines(tmpOut);
                    const mergedCount = writeMergedDanmakuXml(out, [...biliLines, ...selfLines]);
                    try {
                        fs.unlinkSync(tmpOut);
                    }
                    catch (_) { /* 清理失败无害，下次覆盖 */ }
                    log.info(`[biliRunner] 自建源 ${selfLines.length} 条低于阈值(${th}) → 已叠加内置B站 ${biliLines.length} 条，合并去重后 ${mergedCount} 条`);
                    traces.push({
                        id: 'danmu_api', attempted: true, used: true,
                        detail: pre.matched_title
                            ? `命中《${pre.matched_title}》（精确匹配，${selfLines.length} 条低于阈值，已叠加内置B站）`
                            : `精确匹配命中（${selfLines.length} 条低于阈值，已叠加内置B站）`,
                        count: selfLines.length,
                    });
                    traces.push({
                        id: 'bilibili', attempted: true, used: true,
                        detail: biliRes.matched_title ? `命中《${biliRes.matched_title}》` : '匹配成功',
                        count: biliLines.length,
                    });
                    traces.push({
                        id: 'dandanplay', attempted: false, used: false,
                        skippedReason: '前两源已合并命中，未启用（兜底源按需使用）',
                    });
                    return {
                        ...biliRes,
                        danmaku_count: mergedCount,
                        source: 'danmu_api+bilibili',
                        self_count: selfLines.length,
                        bilibili_count: biliLines.length,
                        sources: traces,
                    };
                }
                // B站 链路失败：保留自建源弹幕（out 未被触碰），如实标注失败根因
                log.warn('[biliRunner] 自建源低于阈值但内置B站叠加失败，仅用自建源: ' + ((biliRes && biliRes.error) || '未知'));
                traces.push({
                    id: 'danmu_api', attempted: true, used: true,
                    detail: pre.matched_title ? `命中《${pre.matched_title}》（精确匹配）` : '精确匹配命中',
                    count: pre.danmaku_count,
                });
                traces.push({
                    id: 'bilibili', attempted: true, used: false,
                    error: ((biliRes && biliRes.error) || '未知错误') + '（叠加失败，仅用自建源弹幕）',
                });
                return { ...pre, sources: traces };
            }
        }
        traces.push({
            id: 'danmu_api', attempted: true, used: true,
            detail: pre.matched_title ? `命中《${pre.matched_title}》（精确匹配）` : '精确匹配命中',
            count: pre.danmaku_count,
        });
        // 自建源命中 → 内置 B站 本轮未参与，如实标注（不是失败，是没轮到）
        traces.push({
            id: 'bilibili', attempted: false, used: false,
            skippedReason: '自建源已命中，无需兜底',
        });
        traces.push({
            id: 'dandanplay', attempted: false, used: false,
            skippedReason: '自建源已命中，未启用（兜底源按需使用）',
        });
        return { ...pre, sources: traces };
    }
    // [lc-1266] 失败根因用 danmuApi 的「人话版」：地址不可达（含各地址明细，如节点没运行）、
    // 服务正常但无精确匹配、条目弹幕为空…… 直接显示在 MPV「B站弹幕配置」菜单的
    // 「① 自建弹幕接口」一行——对标「B站 Cookie 状态」的一眼可判，不再只有笼统的「未命中」。
    traces.push({
        id: 'danmu_api', attempted: true, used: false,
        error: danmuApi.consumeLastFailReason() || '未命中（只认精确匹配；未命中即自动降级内置 B站 模糊匹配）',
    });
    // [lc-1117] 网页弹幕设置可单独关掉「B站弹幕搜索」兜底（只影响网页链路；MPV 侧由 Lua 的
    //   bili_search_enabled 门控且不传此参）。手动候选搜索 runBiliDanmakuCandidates 不受限。
    if (!allowBiliFallback) {
        log.info('[biliRunner] B站弹幕搜索未启用（网页弹幕设置），跳过内置 B站降级');
        traces.push({
            id: 'bilibili', attempted: false, used: false,
            skippedReason: '「B站弹幕搜索」已关闭（网页弹幕设置里可重新开启）',
        });
        // [lc-1302] 用户显式关掉 B站 兜底 = 只用自建源：弹弹play 一并跳过（尊重配置，不偷偷扩源）
        traces.push({
            id: 'dandanplay', attempted: false, used: false,
            skippedReason: '「B站弹幕搜索」已关闭，未启用（兜底源按需使用）',
        });
        return { ok: false, error: 'B站弹幕搜索未启用', sources: traces };
    }
    try {
        const r = await runBiliChain(title, ep, out, threshold, season, timeoutMs, altTitle);
        traces.push(r.ok
            ? {
                id: 'bilibili', attempted: true, used: true,
                detail: r.matched_title ? `命中《${r.matched_title}》` : '匹配成功',
                count: r.danmaku_count,
            }
            : { id: 'bilibili', attempted: true, used: false, error: r.error || '未知错误' });
        // [lc-1302] B站 也未命中 → 弹弹play 兜底（官方约定弹幕库按需使用，故置于两源之后；
        //   命中即写 XML 到同一 out，返回同契约结果。失败/未命中保留 B站 的失败根因原样返回。）
        if (!r.ok) {
            const ddp = await dandanplay.autoFetchBili(String(title || ''), Number(ep) || 0, Number(season) || 0, out);
            if (ddp.ok) {
                traces.push({
                    id: 'dandanplay', attempted: true, used: true,
                    detail: ddp.matched_title ? `命中《${ddp.matched_title}》` : '匹配成功',
                    count: ddp.danmaku_count,
                });
                return { ...ddp, sources: traces };
            }
            traces.push({ id: 'dandanplay', attempted: true, used: false, error: ddp.error || '弹弹play 未命中' });
        }
        return { ...r, sources: traces };
    }
    catch (e) {
        log.warn('[biliRunner] run 异常: ' + (e?.message || e));
        traces.push({ id: 'bilibili', attempted: true, used: false, error: String(e?.message || e) });
        return { ok: false, error: String(e?.message || e), sources: traces };
    }
}
/** 仅供测试/诊断：强制下次重新加载模块（例如脚本目录变化后）。 */
function resetBiliModule() {
    cachedModule = null;
    logSinkBound = false;
}
/**
 * [lc-1300] 清除某剧的弹幕源锁定（bili_danmaku.js 的 danmaku_source_cache.json 键）。
 * 网页端「清除弹幕」链路：自动匹配错了 → 清掉记忆，下次自动匹配重新全量搜索/手动重选。
 * @returns 是否确实清掉了已存在的锁定（本来就没锁返回 false，不算失败）
 */
function clearBiliSourceLock(title) {
    try {
        const mod = loadModule();
        if (typeof mod.clear_source_cache !== 'function')
            return false;
        return !!mod.clear_source_cache(title);
    }
    catch (e) {
        log.warn('[biliRunner] 清除弹幕源锁定失败: ' + (e?.message || e));
        return false;
    }
}
/**
 * [lc-1288] B站 Cookie 体检：调 bili_danmaku.js 的 nav 校验（与每次 run 内部同一套逻辑）。
 * 供 playbackShim 的每日定时任务调用，结果写状态文件 + 推送前端详情面板。
 */
async function checkBiliCookie() {
    try {
        const mod = loadModule();
        if (typeof mod.cookie_check !== 'function') {
            return { ok: false, cookie_status: 'error', reason: '脚本无 cookie_check（版本过旧）' };
        }
        const r = await Promise.resolve(mod.cookie_check());
        return (r && typeof r === 'object')
            ? r
            : { ok: false, cookie_status: 'error', reason: 'cookie_check 无返回' };
    }
    catch (e) {
        return { ok: false, cookie_status: 'error', reason: String(e?.message || e) };
    }
}
/**
 * 仅搜索 B站 候选视频列表（标题/bvid/来源/是否合集），不拉取/聚合弹幕。
 * 供 MPV 侧「手动搜索」展示候选列表，由用户选定具体视频。
 * [lc-1261] 双源合并：自建源与 B站 候选同时展示（自建源优先排前）。旧版是二选一
 *   （自建源命中则只出它、未命中才出 B站）——用户手动搜索时看不到全貌：命中时不知道
 *   B站 还有哪些可选，未命中时误以为自建源不支持手动搜索。另附 selfHosted 状态供 UI 说明。
 */
async function runBiliDanmakuCandidates(title, ep, season, timeoutMs = 60000) {
    const selfP = danmuApi
        .candidates(String(title || ''), Number(ep) || 0, Number(season) || 0)
        .catch(() => null);
    // [lc-1302] 弹弹play 候选（ddp:<episodeId> 伪 id）与自建源并列参与手动搜索
    const ddpP = dandanplay
        .candidates(String(title || ''), Number(ep) || 0, Number(season) || 0)
        .catch(() => []);
    const biliP = (async () => {
        let mod;
        try {
            mod = loadModule();
        }
        catch (e) {
            log.warn('[biliRunner] 加载 bili_danmaku.js 失败: ' + (e?.message || e));
            return { ok: false, error: '弹幕脚本加载失败: ' + (e?.message || e) };
        }
        try {
            const runP = Promise.resolve(mod.search_candidates(title, ep, season));
            let timeoutHandle = null;
            const timeoutP = new Promise((resolve) => {
                timeoutHandle = setTimeout(() => resolve({ ok: false, error: `候选搜索超时(${timeoutMs}ms)` }), timeoutMs);
            });
            const r = await Promise.race([runP, timeoutP]);
            if (timeoutHandle)
                clearTimeout(timeoutHandle);
            return (r && typeof r === 'object') ? r : { ok: false, error: '未知错误（search_candidates 无返回）' };
        }
        catch (e) {
            log.warn('[biliRunner] search_candidates 异常: ' + (e?.message || e));
            return { ok: false, error: String(e?.message || e) };
        }
    })();
    const [selfCands, ddpCands, biliRes] = await Promise.all([selfP, ddpP, biliP]);
    const selfList = Array.isArray(selfCands) ? selfCands : [];
    const ddpList = Array.isArray(ddpCands) ? ddpCands : [];
    const biliList = (biliRes && biliRes.ok && Array.isArray(biliRes.candidates))
        ? biliRes.candidates : [];
    const merged = [];
    selfList.forEach((c, i) => { c.index = i; merged.push(c); });
    ddpList.forEach((c, i) => { c.index = selfList.length + i; merged.push(c); });
    biliList.forEach((c, i) => { c.index = selfList.length + ddpList.length + i; merged.push(c); });
    const selfStatus = {
        active: danmuApi.isActive(),
        matched: selfList.length > 0,
        count: selfList.length,
    };
    log.info(`[biliRunner] 候选合并：自建源 ${selfList.length} 个（${selfStatus.active ? '已启用' : '未启用'}） + 弹弹play ${ddpList.length} 个 + B站 ${biliList.length} 个`);
    if (!merged.length) {
        return { ok: false, error: (biliRes && biliRes.error) || '未找到候选', selfHosted: selfStatus };
    }
    return { ok: true, candidates: merged, selfHosted: selfStatus };
}
/**
 * 由用户选定的 bvid 直接拉取该视频弹幕（手动搜索：用户已明确选定视频）。
 */
async function runBiliDanmakuByBvid(title, bvid, out, threshold, epNum = 0, timeoutMs = 60000, forceCid, // [lc-1195] 用户从分P 列表手动选定的 cid（直接用，跳过 ep_num 匹配）
seriesKey = '') {
    // [lc-1101] 用户从候选列表选定的是自建源条目（伪 bvid = `dmapi:<episodeId>`）→ 按 id 直取。
    //   这条分支【不降级】：该 id 不是 B站 bvid，拿给内置链路必然失败，直接回错误更有诊断价值。
    const dmapiId = danmuApi.parsePrefixedId(bvid);
    if (dmapiId)
        return danmuApi.fetchById(dmapiId, String(title || ''), out, seriesKey);
    // [lc-1302] 弹弹play 伪 id（`ddp:<episodeId>`）按 episodeId 直取，同样不降级；
    //   单集 id 不能代表整季，不做源锁定（后续集由自动路径重新匹配）。
    if (String(bvid).startsWith(dandanplay.ID_PREFIX)) {
        const ddpId = Number(String(bvid).slice(dandanplay.ID_PREFIX.length));
        if (!ddpId || ddpId <= 0)
            return { ok: false, error: '弹弹play 候选 id 无效' };
        return dandanplay.fetchById(ddpId, String(title || ''), out);
    }
    let mod;
    try {
        mod = loadModule();
    }
    catch (e) {
        log.warn('[biliRunner] 加载 bili_danmaku.js 失败: ' + (e?.message || e));
        return { ok: false, error: '弹幕脚本加载失败: ' + (e?.message || e) };
    }
    try {
        // [lc-1172] epNum 透传：合集/多P 候选按分P 标题匹配取对应集的 cid
        // [lc-1195] forceCid 透传：用户在分P 明细菜单里手动选定的 cid，直接使用
        const runP = Promise.resolve(mod.run_candidates(title, bvid, out, threshold, epNum, forceCid ? Number(forceCid) : 0));
        let timeoutHandle = null;
        const timeoutP = new Promise((resolve) => {
            timeoutHandle = setTimeout(() => resolve({ ok: false, error: `弹幕获取超时(${timeoutMs}ms)` }), timeoutMs);
        });
        const r = await Promise.race([runP, timeoutP]);
        if (timeoutHandle)
            clearTimeout(timeoutHandle);
        return (r && typeof r === 'object') ? r : { ok: false, error: '未知错误（run_candidates 无返回）' };
    }
    catch (e) {
        log.warn('[biliRunner] run_candidates 异常: ' + (e?.message || e));
        return { ok: false, error: String(e?.message || e) };
    }
}
/**
 * [lc-1195] 列出某合集(bvid)的全部分P（page/cid/part），供 MPV 手动搜索 UI 在点击合集候选后
 * 展开分P 明细菜单，由用户手动选定具体分P（对应脚本内 list_pages，view API 一次请求）。
 */
async function listBiliDanmakuPages(bvid, timeoutMs = 30000) {
    let mod;
    try {
        mod = loadModule();
    }
    catch (e) {
        log.warn('[biliRunner] 加载 bili_danmaku.js 失败: ' + (e?.message || e));
        return { ok: false, error: '弹幕脚本加载失败: ' + (e?.message || e) };
    }
    try {
        const runP = Promise.resolve(mod.list_pages(bvid));
        let timeoutHandle = null;
        const timeoutP = new Promise((resolve) => {
            timeoutHandle = setTimeout(() => resolve({ ok: false, error: `分P 列表超时(${timeoutMs}ms)` }), timeoutMs);
        });
        const r = await Promise.race([runP, timeoutP]);
        if (timeoutHandle)
            clearTimeout(timeoutHandle);
        return (r && typeof r === 'object') ? r : { ok: false, error: '未知错误（list_pages 无返回）' };
    }
    catch (e) {
        log.warn('[biliRunner] list_pages 异常: ' + (e?.message || e));
        return { ok: false, error: String(e?.message || e) };
    }
}
