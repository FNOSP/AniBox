// personWorks.ts — 演员详情页「TMDB 完整作品」注入（lc-1033）
// ─────────────────────────────────────────────────────────────────────────────
// 背景：飞牛演员页只显示库内作品；本插件把 TMDB 全量出演作品（含库外缺失）以与原生
// 作品网格一致的语言追加到页面底部，已入库绿色角标（点击进库内详情），缺失灰化 +
// 「缺失」角标（点击开 TMDB 页）。数据由主进程 person:tmdb-credits 提供
// （personTmdb.ts：飞牛 person/item/list + TMDB combined_credits + owned 匹配）。
// [lc-1040] 防限流：面板默认收起一行头，点开才触发数据查询；海报进视口才下载
// （w342）；数据（主进程 7 天磁盘缓存+SWR）与图片（内存+磁盘）都有持久层，二次点开零网络。
// 参考：第三方油猴插件 fnos-actor-tmdb v4.5（端点与匹配策略经其验证）。
// ─────────────────────────────────────────────────────────────────────────────
import { ipcRenderer } from 'electron';
import { registerHook } from '../core/hooks';
import { pagePath } from '../core/pageMode';
import { HookType } from '../core/hooks';
// [fix-cast-page] 详情页「演职人员」点击 → 人物页的稳定导航（window capture 委托）。
//   放在本插件安装，避免改动 embyWall.ts 的轮播/路由轮询区域（与并行任务隔离）。
import { installCastNav, revealActiveView } from './embyWall/detail/castNav';
// [mirror-error] 演员页 TMDB 取数失败：错误里带 URL/镜像域名时统一显示镜像故障文案。
import { friendlyError } from '../core/mirrorError';


const STYLE_ID = 'fnos-person-works-style';
const PANEL_ID = 'fnos-person-works';
// [lc-1040] 海报尺寸 w92→w342：卡片实际渲染 162px 宽，2x DPI 下需要 324px 源图——
// w92 拉伸两倍就是用户报的「太糊」。w342 是 TMDB 标准档里 2x 的正解；磁盘缓存按完整 URL 存，
// 换尺寸=换 key，首次会一次性重下（旧 w92 条目成为孤儿，无害）。
const POSTER_BASE = 'https://image.tmdb.org/t/p/w342';

const esc = (s: any): string => String(s == null ? '' : s)
  .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;');

// ── 纯渲染（导出供验证复用）──
function cardHtml(m: any): string {
  const owned = !!m.owned;
  const year = m.date ? m.date.slice(0, 4) : '';
  const score = m.score > 0 ? `<span class="fpw-score">${Number(m.score).toFixed(1)}</span>` : '';
  const tvTag = m.media === 'tv' ? '<span class="fpw-tvtag">剧集</span>' : '';
  const missingTag = owned ? '' : '<span class="fpw-missing">缺失</span>';
  const inner = `
    <div class="fpw-poster${owned ? '' : ' missing'}">
      ${m.poster ? `<img alt="" data-path="${esc(m.poster)}">` : '<div class="fpw-noposter">无图</div>'}
      ${score}${missingTag}
    </div>
    <div class="fpw-title">${esc(m.title)}${owned ? '<span class="fpw-owned">已入库</span>' : ''}</div>
    <div class="fpw-sub">${tvTag}${esc(year || '—')}${m.character ? ' · ' + esc(m.character) : ''}</div>`;
  if (owned && m.guid) {
    const route = m.media === 'tv' ? 'tv' : 'movie';
    return `<a class="fpw-card owned" href="/v/${route}/${esc(m.guid)}" data-internal="1">${inner}</a>`;
  }
  const tmdb = `https://www.themoviedb.org/${m.media}/${m.id}`;
  return `<div class="fpw-card missing" data-tmdb="${esc(tmdb)}" title="库内缺失 · 点击查看 TMDB">${inner}</div>`;
}

const PANEL_CSS = `
#${PANEL_ID}{ margin:0 0 10px; }
#${PANEL_ID} .fpw-head{ display:flex; align-items:baseline; gap:10px; margin-bottom:12px; }
#${PANEL_ID} .fpw-h-title{ font-size:15px; font-weight:600; color:var(--semi-color-text-0); }
#${PANEL_ID} .fpw-h-sub{ font-size:12px; color:var(--semi-color-text-2); }
#${PANEL_ID} .fpw-h-toggle{ margin-left:auto; font-size:11.5px; color:var(--semi-color-primary); cursor:pointer; user-select:none; }
#${PANEL_ID} .fpw-h-toggle:hover{ text-decoration:underline; }
#${PANEL_ID} .fpw-h-collapse{ color:var(--semi-color-text-2); }
#${PANEL_ID} .fpw-head.fpw-head-click{ cursor:pointer; }
#${PANEL_ID} .fpw-head.fpw-head-click:hover .fpw-h-title{ color:var(--semi-color-primary); }
#${PANEL_ID} .fpw-grid{ display:flex; flex-wrap:wrap; gap:20px 20px; }
#${PANEL_ID} .fpw-card{ width:162px; cursor:pointer; border-radius:10px; transition:transform .22s ease; }
#${PANEL_ID} .fpw-card:hover{ transform:translateY(-3px); }
#${PANEL_ID} .fpw-card:hover .fpw-title{ color:var(--semi-color-primary); }
#${PANEL_ID} .fpw-poster{ position:relative; width:100%; aspect-ratio:2/3; border-radius:10px; overflow:hidden;
  background:var(--semi-color-fill-0); margin-bottom:8px; }
#${PANEL_ID} .fpw-poster img{ width:100%; height:100%; object-fit:cover; display:block; }
#${PANEL_ID} .fpw-poster.missing img{ filter:grayscale(.65) brightness(.88); opacity:.85; }
#${PANEL_ID} .fpw-noposter{ width:100%; height:100%; display:flex; align-items:center; justify-content:center;
  font-size:11px; color:var(--semi-color-text-2); }
#${PANEL_ID} .fpw-score{ position:absolute; top:8px; left:8px; font-size:11px; font-weight:600;
  color:#fff; background:rgba(0,0,0,.55); padding:1px 6px; border-radius:6px; }
#${PANEL_ID} .fpw-missing{ position:absolute; top:8px; right:8px; font-size:10.5px; font-weight:600;
  color:#ffd7d7; background:rgba(180,40,40,.78); padding:1px 6px; border-radius:6px; }
#${PANEL_ID} .fpw-title{ font-size:13px; line-height:1.35; color:var(--semi-color-text-0);
  overflow:hidden; text-overflow:ellipsis; display:-webkit-box; -webkit-line-clamp:2; -webkit-box-orient:vertical; }
#${PANEL_ID} .fpw-owned{ display:inline-block; margin-left:5px; font-size:10px; font-weight:500;
  padding:0 5px; border-radius:4px; background:var(--semi-color-success); color:#fff; vertical-align:1px; }
#${PANEL_ID} .fpw-sub{ margin-top:3px; font-size:11.5px; color:var(--semi-color-text-2);
  white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
#${PANEL_ID} .fpw-tvtag{ display:inline-block; margin-right:5px; font-size:10.5px; padding:0 5px;
  border-radius:4px; background:var(--semi-color-fill-1); color:var(--semi-color-text-1); }
#${PANEL_ID} .fpw-msg{ font-size:12px; color:var(--semi-color-text-2); padding:14px 2px; }
#${PANEL_ID} .fpw-card.missing, #${PANEL_ID} .fpw-card.owned{ text-decoration:none; }
`;

// ── 运行时 ──
let _styleInjected = false;
let _onlyMissing = false;
let _loadError = '';     // 最近一次加载错误——收起态显示「点击重试」
const _cache = new Map<string, any>();   // guid → credits 响应（渲染进程会话级；持久层在主进程磁盘缓存）
const _inflight = new Set<string>();
let _currentGuid = '';
let _waitingForCol = false;

function ensureStyle(): void {
    if (_styleInjected) return;
    const st = document.createElement('style');
    st.id = STYLE_ID;
    st.textContent = PANEL_CSS;
    (document.head || document.documentElement).appendChild(st);
    _styleInjected = true;
}

function activeCol(): HTMLElement | null {
    // 演员页作品列：唯一 mb-[46px] 容器（person 页结构，活体 2026-09-05）
    const views = document.querySelectorAll('.trim-ui__cache-outlet--exclude');
    for (let i = views.length - 1; i >= 0; i--) {
        const v = views[i] as HTMLElement;
        if (v && v.offsetParent !== null) {
            const col = v.querySelector<HTMLElement>('div[class*="mb-[46px]"]');
            if (col) return col;
        }
    }
    return document.querySelector<HTMLElement>('div[class*="mb-[46px]"]');
}

// ── [fix-person-tmdb] 飞牛人物数据在同源前端抓取 ──────────────────────────────
// 根因：网页端后端转发请求拿不到 httpOnly 会话 Cookie（Trim-MC-token），
//   /v/api/v1/person/{guid} 被判未登录 → data 为空 → 后端只能报「未找到该演员（guid）」。
//   这里改用已验证的「同源 fetch + credentials:include + Authx 签名」路径（jav/海报同款），
//   把 name / imdbId / tmdbId / 库内作品 一并交给后端，后端只做 TMDB 解析与 owned 匹配。
function fnNonce(): string {
    return String(Math.floor(Math.random() * 900000) + 100000);
}

async function fnosJson(method: 'GET' | 'POST', path: string, body?: any): Promise<any> {
    const headers: Record<string, string> = {};
    try {
        const authx = await ipcRenderer.invoke('fnos-gen-authx', path, body);
        if (authx) headers.Authx = String(authx);
    } catch { /* 签名不可用时仍尝试同源 cookie 请求 */ }
    const init: any = { method, credentials: 'include', headers };
    if (body !== undefined) {
        headers['Content-Type'] = 'application/json';
        init.body = JSON.stringify(body);
    }
    const resp = await fetch(location.origin + path, init);
    if (!resp.ok) throw new Error('HTTP ' + resp.status);
    return resp.json();
}

/** 演员名 DOM 兜底：document.title 或活跃视图里的大标题。 */
function personNameFromDom(): string {
    const t = String(document.title || '').split(/\s+[-–—|]\s+/)[0].trim();
    if (t && !/飞牛|影视|AniBox|FnTV/i.test(t)) return t;
    const views = document.querySelectorAll<HTMLElement>('.trim-ui__cache-outlet--exclude');
    for (let i = views.length - 1; i >= 0; i--) {
        if (!views[i] || views[i].offsetParent === null) continue;
        const h = views[i].querySelector<HTMLElement>('h1, h2');
        const txt = h ? String(h.textContent || '').trim() : '';
        if (txt) return txt;
    }
    return '';
}

/** 汇总飞牛人物提示：详情（name/imdb/tmdb id）+ 库内作品（owned 匹配键）。 */
async function collectPersonHints(guid: string): Promise<any> {
    const hints: any = { guid };
    try {
        const pd = await fnosJson('GET', '/v/api/v1/person/' + guid);
        const p = pd && pd.data && typeof pd.data === 'object' ? pd.data : null;
        if (p) {
            hints.name = String(p.name || p.title || '').trim();
            hints.imdbId = String(p.imdb_id || p.imdbId || '').trim();
            const nested = (p.person && (p.person.tmdb_id || p.person.tmdbId))
                || (p.external_ids && (p.external_ids.tmdb_id || p.external_ids.tmdbId));
            const tid = Number(p.tmdb_id || p.tmdbId || nested || 0);
            if (tid > 0) hints.tmdbId = tid;
        }
    } catch { /* 详情失败 → DOM 姓名兜底 */ }
    if (!hints.name) hints.name = personNameFromDom();

    const lib: any[] = [];
    const seen = new Set<string>();
    // [perf-person] 5 个 job 并发拉取（旧实现逐个串行 await，5 次 RTT 叠加），
    // 仍按原 job 顺序合并 + 去重，结果与串行版一致。
    const lists = await Promise.all(['Actor', 'Director', 'Writer', 'Screenplay', 'Producer'].map(async (job) => {
        try {
            const body = {
                person_guid: guid, page: 1, page_size: 200, job,
                sort_column: 'update_time', sort_type: 'desc', nonce: fnNonce(),
            };
            const j = await fnosJson('POST', '/v/api/v1/person/item/list', body);
            return j && j.data && Array.isArray(j.data.list) ? j.data.list : [];
        } catch { return []; /* 单 job 失败忽略 */ }
    }));
    for (const list of lists) {
        for (const it of list) {
            const title = String(it.title || it.name || it.original_title || it.original_name || '').trim();
            if (!title) continue;
            const key = String(it.guid || '') + '|' + title;
            if (seen.has(key)) continue;
            seen.add(key);
            lib.push({
                title,
                type: String(it.type || it.media_type || '').toLowerCase(),
                guid: String(it.guid || ''),
                tmdbId: Number(it.tmdb_id || it.tmdbId || 0) || undefined,
                imdbId: String(it.imdb_id || it.imdbId || ''),
            });
            if (lib.length >= 600) break;
        }
        if (lib.length >= 600) break;
    }
    if (lib.length) hints.libItems = lib;
    return hints;
}

function renderPanel(container: HTMLElement, data: any): void {
    let box = container.querySelector<HTMLDivElement>('#' + PANEL_ID);
    if (!box) {
        box = document.createElement('div');
        box.id = PANEL_ID;
        container.appendChild(box); // 追加为作品列末尾（原生「作为演员」网格之后）
    }
    if (box.dataset.guid !== _currentGuid) { box.innerHTML = ''; box.dataset.guid = _currentGuid; }
    ensureStyle();
    const items: any[] = data.items || [];
    const ownedCount = Number(data.ownedCount || 0);
    const list = _onlyMissing ? items.filter((m) => !m.owned) : items;
    const head = `
      <div class="fpw-head">
        <span class="fpw-h-title">TMDB 完整作品</span>
        <span class="fpw-h-sub">共 ${items.length} 部 · 库内 ${ownedCount} 部</span>
        <span class="fpw-h-toggle fpw-h-collapse">收起</span>
        <span class="fpw-h-toggle fpw-h-missing-toggle">${_onlyMissing ? '显示全部' : '仅看缺失'}</span>
      </div>`;
    const grid = `<div class="fpw-grid">${list.map(cardHtml).join('') || '<div class="fpw-msg">没有缺失作品 ✓ 全部已入库</div>'}</div>`;
    box.innerHTML = head + grid;

    const toggle = box.querySelector('.fpw-h-missing-toggle');
    if (toggle) toggle.addEventListener('click', () => { _onlyMissing = !_onlyMissing; renderPanel(container, data); });
    // [lc-1040] 收起：数据留在内存缓存，再点开毫秒级还原、零网络
    const collapse = box.querySelector('.fpw-h-collapse');
    if (collapse) collapse.addEventListener('click', () => { renderCollapsed(container); });

    // [lc-1040] 海报懒加载：进视口才走主进程 tmdb:image 代理（内存+磁盘缓存，规避 DNS 污染；
    // 复用详情页 TMDB 卡管线）——不再百张齐发，滚动到哪下载到哪
    observePosters(box);
    // 缺失卡：点击开 TMDB 页（库内卡是 <a>，由 fnOS 路由接管）
    box.querySelectorAll<HTMLElement>('.fpw-card.missing').forEach((el) => {
        el.addEventListener('click', (ev: Event) => {
            ev.preventDefault();
            const u = el.getAttribute('data-tmdb');
            if (!u) return;
            // [fix-cast-page] 网页端 shim 的 shell.openExternal 是空实现（不抛错）→ 旧代码
            //   try{shell.openExternal(u)}catch{window.open} 的兜底永远不触发，点击缺失作品毫无反应。
            //   改走统一的 app:open-external（网页端=window.open 新标签，桌面端=系统浏览器）。
            try {
                const p = ipcRenderer.invoke('app:open-external', u);
                if (p && typeof p.catch === 'function') p.catch(() => { try { window.open(u, '_blank'); } catch { /* ignore */ } });
            } catch { try { window.open(u, '_blank'); } catch { /* ignore */ } }
        });
    });
}

function renderMsg(container: HTMLElement, msg: string): void {
    ensureStyle();
    let box = container.querySelector<HTMLDivElement>('#' + PANEL_ID);
    if (!box) { box = document.createElement('div'); box.id = PANEL_ID; container.appendChild(box); }
    box.dataset.guid = _currentGuid;
    box.innerHTML = `<div class="fpw-head"><span class="fpw-h-title">TMDB 完整作品</span></div><div class="fpw-msg">${esc(msg)}</div>`;
}

/** [lc-1040] 收起态：只有一行头，零网络请求（进演员页不再自动拉数据/下图片）。
 *  点击头部才真正查询/下载；有错误时同位置显示「点击重试」。 */
function renderCollapsed(container: HTMLElement): void {
    ensureStyle();
    let box = container.querySelector<HTMLDivElement>('#' + PANEL_ID);
    if (!box) { box = document.createElement('div'); box.id = PANEL_ID; container.appendChild(box); }
    box.dataset.guid = _currentGuid;
    box.innerHTML = '<div class="fpw-head fpw-head-click" role="button">'
        + '<span class="fpw-h-title">TMDB 完整作品</span>'
        + `<span class="fpw-h-sub">${_loadError ? esc(_loadError) : '未加载 · 点击后查询 TMDB'}</span>`
        + `<span class="fpw-h-toggle">${_loadError ? '点击重试' : '点击加载'}</span></div>`;
    const head = box.querySelector('.fpw-head');
    if (head) head.addEventListener('click', () => { void expandCurrent(); });
}

/** [lc-1040] 点开才触发查询/下载（用户要求防限流）：数据走主进程磁盘缓存（lc-1039，7 天+SWR），
 *  二次点开毫秒级零网络；海报则由 observePosters 按“进视口”逐批下载。 */
async function expandCurrent(retry = 0): Promise<void> {
    const guid = _currentGuid;
    if (!guid) return;
    if (_inflight.has(guid)) return;
    // [fix-cast-page] 不再静默失败：activeCol() 可能因作品列被 React 重建/路由切换而瞬时取不到，
    //   此时回落到面板自身的父容器（面板头就在里面），仍取不到则给出可见提示并安排一次重试，
    //   而不是「点了没反应」。
    let container = activeCol();
    if (!container) {
        const box = document.getElementById(PANEL_ID);
        container = (box && box.parentElement) as HTMLElement | null;
    }
    if (!container) {
        log.warn('[personWorks] 展开失败：未找到演员页作品列');
        if (retry < 2) setTimeout(() => { if (_currentGuid === guid) void expandCurrent(retry + 1); }, 800);
        return;
    }
    const cached = _cache.get(guid);
    if (cached) { renderPanel(container, cached); return; }
    _inflight.add(guid);
    _loadError = '';
    renderMsg(container, '正在读取演员信息并从 TMDB 拉取完整作品…');
    try {
        // [fix-person-tmdb] 先在页面同源抓取飞牛人物详情/库内作品（带 httpOnly 会话），
        // 再把提示交给后端做 TMDB 解析——不再让后端用 guid 直接请求飞牛（必失败）。
        let hints: any = { guid };
        try {
            hints = await collectPersonHints(guid);
        } catch (e: any) {
            log.warn('[personWorks] 飞牛人物信息读取失败，仅按 guid 请求：', e && e.message);
        }
        if (_currentGuid !== guid) return;

        let r: any = null;
        try {
            r = await ipcRenderer.invoke('person:tmdb-credits', hints);
        } catch (err: any) {
            const msg = String((err && err.message) || err);
            // [lc-1034] 主进程插件未加载（常见于只刷新了页面、主进程还是旧版）→ 给出可执行指引
            if (msg.indexOf('No handler registered') !== -1) {
                renderMsg(container, '主进程尚未加载本插件——请完整重启客户端（退出进程再启动，仅刷新页面无效）后重试');
                return;
            }
            throw err;
        }
        if (_currentGuid !== guid) return; // 已切页
        if (!r || r.error) { _loadError = friendlyError(r && r.error, '拉取失败'); renderCollapsed(container); return; }
        // [fix-person-tmdb] 未关联 TMDB 人物是明确的空状态，不是加载失败：不缓存、可重试。
        if (r.unlinked || r.noImdb) {
            renderMsg(container, unlinkedMessage(r));
            return;
        }
        _cache.set(guid, r);
        if (!Array.isArray(r.items) || !r.items.length) {
            renderMsg(container, 'TMDB 暂无该演员的作品记录');
            return;
        }
        renderPanel(container, r);
    } catch (e: any) {
        if (_currentGuid === guid) {
            // [mirror-error] 原文含 URL/镜像域名 → 统一镜像故障文案；其余保留原文便于操作。
            _loadError = friendlyError(e, '拉取失败');
            renderCollapsed(container); // 错误态回落收起头部，点击即重试
        }
    } finally {
        _inflight.delete(guid);
    }
}

/** [fix-person-tmdb] 未关联时的可区分文案（不再显示成「未找到该演员（guid）」）。 */
function unlinkedMessage(r: any): string {
    const name = String((r && r.name) || '').trim();
    const who = name ? `「${name}」` : '该演员';
    switch (r && r.reason) {
        case 'no-name':
            return '未关联 TMDB 人物：未能读取演员姓名，无法在 TMDB 检索';
        case 'no-external-id':
            return `未关联 TMDB 人物：飞牛未提供 ${who} 的 TMDB/IMDb 关联，按姓名也未在 TMDB 找到匹配`;
        default:
            return `未关联 TMDB 人物：TMDB 未收录${who}`;
    }
}

let _posterIO: IntersectionObserver | null = null;

function loadPoster(img: HTMLImageElement): void {
    const p = img.getAttribute('data-path') || '';
    if (!p) return;
    const u = POSTER_BASE + p;
    ipcRenderer.invoke('tmdb:image', u)
        .then((r: any) => { if (r && r.ok && r.dataUrl && document.body.contains(img)) img.src = r.dataUrl; })
        .catch(() => { if (document.body.contains(img)) img.src = u; }); // 代理失败退直链
}

/** [lc-1040] 视口内才下载海报（rootMargin 预拉约一屏）；「仅看缺失」切换重渲染后对新卡重新观察。
 *  主进程内存+磁盘双层缓存兜底：同一张图整个软件生命周期只真正下载一次。 */
function observePosters(box: HTMLElement): void {
    if (_posterIO) { _posterIO.disconnect(); _posterIO = null; }
    const imgs = Array.from(box.querySelectorAll<HTMLImageElement>('img[data-path]'));
    if (typeof IntersectionObserver === 'undefined') { imgs.forEach(loadPoster); return; }
    _posterIO = new IntersectionObserver((entries) => {
        for (const en of entries) {
            if (!en.isIntersecting) continue;
            const img = en.target as HTMLImageElement;
            if (_posterIO) _posterIO.unobserve(img);
            loadPoster(img);
        }
    }, { rootMargin: '300px 0px' });
    imgs.forEach((im) => { if (_posterIO) _posterIO.observe(im); });
}

/** 有界等待作品列渲染（原生「作为演员」网格就绪后再注入，避免插早被 React 冲掉）。
 *  [lc-1040] 只注入收起态头部，不自动拉取。 */
function waitForColumn(guid: string, attempt: number): void {
    if (_currentGuid !== guid) return; // 已切走
    const col = activeCol();
    if (col && col.children.length >= 2) {
        renderCollapsed(col);
        return;
    }
    if (attempt >= 10) {  return; }
    setTimeout(() => waitForColumn(guid, attempt + 1), 600);
}

/** [lc-1036] 二级详情页（季页）演职人员行富化：逐演员拉 TMDB 简报
 *  （职业分类/生日/代表作前二），注入行右缘补充信息槽（beautifyStyle E2 ④b 槽位）。 */
const _castEnrichBusy = { v: false };
function enrichCast(): void {
    if (_castEnrichBusy.v) return;
    const views = document.querySelectorAll('.trim-ui__cache-outlet--exclude');
    let view: HTMLElement | null = null;
    for (let i = views.length - 1; i >= 0; i--) {
        const v = views[i] as HTMLElement;
        if (v && v.offsetParent !== null && v.querySelector('a[href*="/v/person/"]')) { view = v; break; }
    }
    if (!view) return;
    const links = Array.from(view.querySelectorAll('a[href*="/v/person/"]')) as HTMLAnchorElement[];
    const todo = links.filter((a) => !a.dataset.fpwEnriched);
    if (!todo.length) return;
    _castEnrichBusy.v = true;
    todo.forEach((a, i) => {
        a.dataset.fpwEnriched = String(i);
        const m = (a.getAttribute('href') || '/').match(/([0-9a-f]{32})/i);
        if (!m) return;
        const guid = m[1].toLowerCase();
        // [fix-person-tmdb] 带上姓名：后端无法直连飞牛详情时仍能按姓名解析 TMDB 人物。
        const nameEl = a.querySelector('p[class*="text-base"]');
        const personName = nameEl ? String(nameEl.textContent || '').trim() : '';
        setTimeout(() => {
            ipcRenderer.invoke('person:tmdb-brief', { guid, name: personName }).then((b: any) => {
                if (!b || b.error || !document.body.contains(a)) return;
                const parts: string[] = [];
                // [lc-1036] 原生角色文案已含职业分类（如角色 p 就是「演员」）时不重复推送
                const roleEl = a.querySelector('p:not([class*="text-base"])');
                const roleTxt = roleEl ? (roleEl.textContent || '').trim() : '';
                if (b.dept && roleTxt.indexOf(b.dept) === -1) parts.push(b.dept);
                if (b.birthday) parts.push(b.birthday);
                if (b.top && b.top.length) parts.push(b.top.join('、'));
                if (!parts.length) return;
                const span = document.createElement('span');
                span.className = 'fn-cast-extra';
                span.textContent = parts.join(' · ');
                span.title = span.textContent;
                a.appendChild(span);
            }).catch(() => {});
        }, i * 180);
    });
    setTimeout(() => { _castEnrichBusy.v = false; }, 900);
}

function checkRoute(): void {
    // [lc-1036] 二级详情页（季页）→ 演职人员行富化
    if (/\/v\/(?:tv|movie)\/season\/[0-9a-f]{32}/i.test(pagePath())) enrichCast();
    const m = pagePath().match(/\/v\/person\/([0-9a-f]{32})/i);
    if (!m) {
        if (_currentGuid) {
            _currentGuid = '';
            _loadError = '';   // [lc-1040] 换页复位；观察器随面板 DOM 移除自动失效
            if (_posterIO) { _posterIO.disconnect(); _posterIO = null; }
            const b = document.getElementById(PANEL_ID);
            if (b && b.parentElement) b.parentElement.removeChild(b);
        }
        return;
    }
    const guid = m[1].toLowerCase();
    // [fix-cast-page] 人物页已打开：确保它没有被视图清理逻辑(hideStaleViews)隐藏。
    //   点击委托在导航后 450/900/1400ms 已复原一次；这里每轮询再兜一层，
    //   覆盖「人物页渲染晚于 1.4s」的情况。
    revealActiveView();
    if (guid === _currentGuid) return;
    _currentGuid = guid;
    _onlyMissing = false;
    _loadError = '';   // [lc-1040] 每个演员页都从收起态开始（点开才拉）
    waitForColumn(guid, 0);
}

function handle(): void {
    try {
        // [fix-cast-page] 演职人员点击委托（window capture）——只认 /v/person/<guid>，
        //   抢在网关 shim 的整页跳转与原生 SPA 处理器之前接管，保证人物页一定打开。
        installCastNav();
        setInterval(checkRoute, 1200);
        checkRoute();
        
    } catch (e: any) { log.warn('初始化失败:', e && e.message); }
}
registerHook(HookType.OnReady, handle);

// 供验证工装复用（纯渲染）
export const __test = { cardHtml, PANEL_CSS };
export {};
