// embyWall/detail/castNav.ts — 详情页「演职人员」点击 → 人物页 稳定导航
// ─────────────────────────────────────────────────────────────────────────────
// 病灶（用户报障）：剧集详情页「演职人员」列表正常显示（头像/姓名/角色），但点击某个人物
//   没有任何反应——不跳人物页、不弹窗。列表本身是 fnOS 原生的
//   `a[href*="/v/person/<guid>"]`，点击本应由 SPA 路由接管，但实际链路被截断：
//   · 网关模式（fpk 经 /app/fntvplus 反代）下，`inject.go` 的 gatewayShim 在 document
//     capture 阶段拦所有 `a[href="/v/*"]` 并改走 `location.assign()`（整页跳转）；
//   · 桌面/直连模式下交给 fnOS SPA，但本插件对 history.pushState/popstate 的包装 +
//     导航后的 hideStaleViews 视图清理，可能在人物页渲染前后把新视图 display:none；
//   · 两条链路都依赖「原生点击处理器一定在、且事件能到达目标」——任一环被吞就表现为
//     「点了没反应」。
//
// 修法：在 **window capture** 阶段（早于 document capture 的 gatewayShim、也早于目标上的
//   React 处理器）统一接管演职人员链接的点击：
//   ① 只认 `/v/person/<32hex>`，其余链接一律放行（不碰卡片/轮播/选集）；
//   ② 用本项目已验证的 spaNav 方式导航（history.pushState + 派发 popstate），
//      网关前缀由 shim 的 pushState 包装补回；失败/SPA 未接管时整页兜底；
//   ③ 导航后一小段时间内持续「复原被 hideStaleViews 隐藏的活跃视图」，保证人物页不被
//      视图清理逻辑立刻隐藏（display:none 是非 important 的内联样式，清掉即可恢复）。
//
// 安装点：personWorks.ts 的 OnReady（该插件本就负责人物页链路，避免改 embyWall.ts 的
//   轮播/路由轮询区域，降低与并行任务冲突的概率）。
// ─────────────────────────────────────────────────────────────────────────────
import { pagePath, tvHref } from '../../../core/pageMode';
import { log } from '../log';

/** `/v/person/<32hex>`（允许网关前缀由 pagePath 处理，这里只取原始 href 的 guid）。 */
const PERSON_HREF_RE = /\/v\/person\/([0-9a-f]{32})/i;
const PERSON_PATH_RE = /^\/v\/person\/[0-9a-f]{32}/i;
const ACTIVE_VIEW_SEL = '.trim-ui__cache-outlet--exclude';
/** 详情页 hero（与 glass.ts DETAIL_HERO_SEL 同义，用于判断「SPA 是否真的换页了」）。 */
const DETAIL_HERO_SEL = ':is(.semi-always-dark[class*="h-[470px]"],.semi-always-dark[class*="min-h-[390px]"],.trim-mc__details--key-version)';

/** 同一次导航的防重入窗口（双击/连点）。 */
let _busyUntil = 0;

function personHref(guid: string): string {
  // tvHref 只在网关模式下补 /app/fntvplus 前缀；直连模式原样返回。
  return tvHref('/v/person/' + guid);
}

/** 当前活跃视图（末尾可见的 --exclude；与 glass.ts findActiveDetailView 同口径）。 */
function activeOutlet(): HTMLElement | null {
  const list = document.querySelectorAll<HTMLElement>(ACTIVE_VIEW_SEL);
  for (let i = list.length - 1; i >= 0; i--) {
    if (list[i] && list[i].offsetParent !== null) return list[i];
  }
  return null;
}

/** 复原被 hideStaleViews 打上的内联 display:none（它用的是非 important 的 display:none，
 *  只作用在「全屏 absolute 视图」上）。分两处查：
 *   ① 活跃视图自身及其祖先被隐藏（人物页整个 outlet 被 display:none）；
 *   ② 活跃视图内部的全屏 absolute 子视图被隐藏（视图栈里的人物页视图）。
 *  只清 inline display:none 且 computed position 为 absolute 的节点——正是 hideStaleViews
 *  的写入目标，不会误放行人物页里本来隐藏的小浮层。 */
export function revealActiveView(): void {
  const outlets = Array.from(document.querySelectorAll<HTMLElement>(ACTIVE_VIEW_SEL));
  if (!outlets.length) return;
  for (const outlet of outlets) {
    let el: HTMLElement | null = outlet;
    for (let i = 0; el && el !== document.body && i < 8; i++) {
      if (el.style && el.style.display === 'none') {
        el.style.display = '';
        log('castNav: 复原被视图清理隐藏的活跃视图 | class=', String(el.className).slice(0, 40));
      }
      el = el.parentElement;
    }
    outlet.querySelectorAll<HTMLElement>('[style*="display"]').forEach((child) => {
      if (!child.style || child.style.display !== 'none') return;
      if (child.closest('[id^="fnos-"], [id^="fntv-"]')) return; // 我们自己的注入层不动
      let pos = '';
      try { pos = getComputedStyle(child).position; } catch { /* ignore */ }
      if (pos !== 'absolute') return;
      child.style.display = '';
      log('castNav: 复原被视图清理隐藏的子视图 | class=', String(child.className).slice(0, 40));
    });
  }
}

/** SPA 是否已经切到人物页（URL 变了但视图仍是详情 hero → 视为未接管）。 */
function stillShowingDetail(): boolean {
  const active = activeOutlet();
  if (!active) return false;
  return !!active.querySelector(DETAIL_HERO_SEL);
}

/** 导航到人物页：SPA 优先（pushState + popstate，与轮播/原生列表同路径），失败整页兜底。 */
function goPerson(guid: string): void {
  const now = Date.now();
  if (now < _busyUntil) return;
  _busyUntil = now + 900;
  const path = '/v/person/' + guid;
  const href = personHref(guid);
  let pushed = false;
  try {
    history.pushState({}, '', path);
    window.dispatchEvent(new PopStateEvent('popstate'));
    pushed = true;
  } catch (e: any) {
    log('castNav: pushState 失败，整页兜底 |', e && e.message);
  }
  if (!pushed) {
    try { location.assign(href); } catch { /* ignore */ }
    return;
  }
  // 路由已切换：持续一小段时间复原视图（hideStaleViews 在导航后 ~400ms 才跑）。
  // 900ms 时若仍看到详情 hero（SPA 未真正换页）→ 整页兜底，确保人物页一定打开。
  [450, 900, 1400].forEach((ms) => {
    window.setTimeout(() => {
      if (!PERSON_PATH_RE.test(pagePath())) return;
      revealActiveView();
      const notRendered = stillShowingDetail() || (ms >= 1400 && !activeOutlet());
      if (ms >= 900 && notRendered) {
        log('castNav: 人物页未渲染，整页兜底 →', href);
        try { location.assign(href); } catch { /* ignore */ }
      }
    }, ms);
  });
}

/** 安装 window 级 capture 委托（幂等，跨 SPA 路由常驻）。 */
export function installCastNav(): void {
  if (typeof window === 'undefined') return;
  if ((window as any).__fntvCastNav) return;
  (window as any).__fntvCastNav = '1';

  // window capture 先于 document capture：抢在 gatewayShim 的 <a href="/v/*"> 整页跳转
  // 与 React 原生 onClick 之前接管，事件链只剩我们这一条，避免双重导航。
  window.addEventListener('click', (e: MouseEvent) => {
    if (e.defaultPrevented) return;
    if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return; // 新标签/修饰键放行
    const t = e.target as Element | null;
    if (!t || typeof (t as any).closest !== 'function') return;
    const a = (t as Element).closest('a[href*="/v/person/"]') as HTMLAnchorElement | null;
    if (!a) return;
    if (a.target && a.target !== '_self') return; // 显式新窗口放行
    const m = (a.getAttribute('href') || '').match(PERSON_HREF_RE);
    if (!m) return;
    const guid = m[1].toLowerCase();
    // 拦下事件：不冒泡到 document（gatewayShim / 汉堡 / 首页兜底委托）与目标（React）。
    e.preventDefault();
    e.stopPropagation();
    log('castNav: 点击演职人员 → /v/person/' + guid);
    goPerson(guid);
  }, true);

  log('castNav: 演职人员点击委托已安装(window capture)');
}
