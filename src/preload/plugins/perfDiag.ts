// preload/plugins/perfDiag.ts
// [lc-1284] 性能诊断：FPS 悬浮表 + 玻璃滤镜开关 —— 定位「桌面版卡、fpk 不卡」的壳开销。
//   Ctrl+Shift+Alt+F  开/关 FPS 帧诊断悬浮表
//   Ctrl+Shift+Alt+B  开/关 玻璃滤镜（全页 backdrop-filter 强制 none）
// [lc-1285] 热键改三键组合：原 Ctrl+Alt+F/B 与用户已有软件冲突。
// [lc-1344] 徽章升级为帧诊断：Long Animation Frames 慢帧归因。
// [lc-1345] 实测归因：599项页慢帧 JS 138ms(DIV.onscroll·飞牛原生虚拟列表)样式/布局0ms；
//   普通页「无慢帧却全帧33ms」→ 嫌疑指向渲染路径(VizDisplayCompositor被禁/GPU光栅未生效)。
//   徽章新增: ①GPU特征状态(主进程 app.getGPUFeatureStatus 经IPC取样一次) ②慢帧滚动目标
//   采样(scrollTarget: 慢帧期间哪个元素在滚) ③帧间隔直方分布(判"统一半速"vs"尖峰卡顿")。
// 默认零开销：未开 FPS 时不跑 rAF、不挂观察器，热键仅一个捕获监听器。
import { ipcRenderer } from 'electron';
import { registerHook, HookType } from '../core/hooks';
import logger from '../core/logger';

const FPS_KEY = 'fntv:diag-fps';
const NOGLASS_KEY = 'fntv:diag-noglass';
const STYLE_ID = 'fntv-diag-noglass-style';
const BADGE_ID = 'fntv-diag-fps-badge';
const HINT_ID = 'fntv-diag-hint';

function lsOn(key: string): boolean {
    try { return localStorage.getItem(key) === '1'; } catch { return false; }
}
function lsSet(key: string, on: boolean): void {
    try { if (on) localStorage.setItem(key, '1'); else localStorage.removeItem(key); } catch { /* ignore */ }
}

function transparentWindowOn(): boolean {
    try { return process.env.FNTV_NO_TRANSPARENT !== '1'; } catch { return true; }
}

// 玻璃关闭态必须赶在首帧前生效 → 模块级立即注入（铁律例外；try/catch 保证不致插件加载失败，
// head 未就绪时由 OnReady 兜底再注入一次）
function applyNoGlass(): void {
    try {
        if (typeof document === 'undefined') return;
        const on = lsOn(NOGLASS_KEY);
        let s = document.getElementById(STYLE_ID) as HTMLStyleElement | null;
        if (on) {
            if (!s) {
                s = document.createElement('style');
                s.id = STYLE_ID;
                (document.head || document.documentElement).appendChild(s);
            }
            s.textContent = '*{backdrop-filter:none!important;-webkit-backdrop-filter:none!important}';
        } else if (s) {
            s.remove();
        }
    } catch (e) {
        logger.warn('[perfDiag] applyNoGlass 失败:', e);
    }
}

// ── GPU 特征状态（lc-1345: 主进程一次性取样） ──
let gpuStatus = '';
function fetchGpuStatus(): void {
    try {
        ipcRenderer.invoke('perf-diag-gpu-status').then((s: any) => {
            if (typeof s === 'string' && s) gpuStatus = s;
        }).catch(() => { /* 主进程无此通道时留空 */ });
    } catch { /* ignore */ }
}
function shortGpu(): string {
    if (!gpuStatus) return '未知';
    // 取关键项: aggregated compositing/rasterization/display_compositor
    const pick = (k: string): string => {
        const m = gpuStatus.match(new RegExp(k + '\\\\?[^\\n]*?:\\s*([a-z_]+)', 'i'));
        return m ? m[1] : '';
    };
    const parts: string[] = [];
    const comp = pick('Compositing') || pick('compositing');
    const raster = pick('Rasterization') || pick('rasterization');
    const gl = pick('WebGL');
    if (comp) parts.push('合成:' + comp);
    if (raster) parts.push('光栅:' + raster);
    if (gl) parts.push('WebGL:' + gl);
    return parts.length ? parts.join(' ') : gpuStatus.slice(0, 60).replace(/\n/g, ' ');
}

// ── FPS 悬浮表（含慢帧归因） ──
let badge: HTMLDivElement | null = null;
let rafId = 0;
let lastTs = 0;
let windowStart = 0;
let frames = 0;
let worstMs = 0;
let loafObserver: PerformanceObserver | null = null;

// 500ms 窗口内慢帧(>32ms)归因累计
const loaf = { long: 0, topScriptMs: 0, topScript: '', styleMs: 0, layoutMs: 0, scrollTarget: '' };
function resetLoafWindow(): void {
    loaf.long = 0; loaf.topScriptMs = 0; loaf.topScript = ''; loaf.styleMs = 0; loaf.layoutMs = 0; loaf.scrollTarget = '';
}
// 慢帧期间的滚动容器采样: 慢帧结束时刻读 document.scrollingElement 与各候选容器的 scrollTop 变化
let lastScrollProbe = { top: -1, el: '' };
function probeScrollTarget(): void {
    try {
        const se = document.scrollingElement as HTMLElement | null;
        const candidates: Element[] = [];
        if (se) candidates.push(se);
        document.querySelectorAll('div,section,main').forEach((el) => {
            if (candidates.length > 24) return;
            const cs = getComputedStyle(el);
            if (cs.overflowY === 'auto' || cs.overflowY === 'scroll') candidates.push(el);
        });
        for (const el of candidates) {
            const st = (el as HTMLElement).scrollTop;
            if (st !== lastScrollProbe.top && lastScrollProbe.top >= 0) {
                const id = el === se ? 'document' : (el.className ? String(el.className).slice(0, 40) : el.tagName);
                loaf.scrollTarget = id + ' @' + st;
            }
            lastScrollProbe = { top: st, el: '' };
        }
    } catch { /* ignore */ }
}
function ensureLoafObserver(): void {
    if (loafObserver) return;
    try {
        loafObserver = new PerformanceObserver((list) => {
            for (const raw of list.getEntries()) {
                const e = raw as any;
                if ((e.duration || 0) < 32) continue; // 只看超过 2 帧的慢帧
                loaf.long++;
                loaf.styleMs += (e.styleRecalcDuration || 0);
                loaf.layoutMs += (e.layoutDuration || 0);
                let top: any = null;
                for (const s of (e.scripts || [])) {
                    if (!top || (s.duration || 0) > (top.duration || 0)) top = s;
                }
                if (top && (top.duration || 0) > loaf.topScriptMs) {
                    loaf.topScriptMs = top.duration || 0;
                    const fn = top.sourceFunctionName || top.invoker || '';
                    const url = String(top.sourceURL || '');
                    const base = url ? url.split(/[\\/]/).pop() : '';
                    loaf.topScript = (fn ? fn : (base || 'script')).slice(0, 30);
                }
                probeScrollTarget();
            }
        });
        loafObserver.observe({ type: 'long-animation-frame', buffered: false } as any);
    } catch {
        loafObserver = null;
    }
}

function ensureBadge(): HTMLDivElement {
    if (badge && badge.isConnected) return badge;
    badge = document.createElement('div');
    badge.id = BADGE_ID;
    badge.setAttribute('data-fnos-ui', '1'); // 白底清除器/a11y/pageAnim 豁免标记
    badge.style.cssText = 'position:fixed;top:40px;right:12px;z-index:2147483647;pointer-events:none;'
        + 'background:rgba(10,12,18,.74);color:#7CFC9A;font:600 12px/1.55 Consolas,monospace;'
        + 'padding:5px 10px;border-radius:8px;text-align:right;white-space:pre;';
    document.body.appendChild(badge);
    return badge;
}

function fmtBadge(fps: number): string {
    const lines: string[] = [];
    lines.push(`${fps} FPS · 最差 ${Math.round(worstMs)}ms`);
    lines.push(`透明窗:${transparentWindowOn() ? '开' : '关(诊断)'} · 玻璃:${lsOn(NOGLASS_KEY) ? '关(测试)' : '开'}`);
    if (loaf.long > 0) {
        lines.push(`慢帧x${loaf.long}: JS ${Math.round(loaf.topScriptMs)}ms(${loaf.topScript || '?'})`);
        lines.push(`  样式 ${Math.round(loaf.styleMs)}ms · 布局 ${Math.round(loaf.layoutMs)}ms${loaf.scrollTarget ? ' · 滚:' + loaf.scrollTarget : ''}`);
    } else {
        lines.push('无慢帧(≤32ms)');
    }
    lines.push('GPU: ' + shortGpu());
    return lines.join('\n');
}

function tick(ts: number): void {
    if (!lastTs) {
        lastTs = ts; windowStart = ts; frames = 0; worstMs = 0;
    } else {
        const dt = ts - lastTs;
        lastTs = ts;
        if (dt > worstMs) worstMs = dt;
        frames++;
        const elapsed = ts - windowStart;
        if (elapsed >= 500) {
            ensureBadge().textContent = fmtBadge(Math.round(frames * 1000 / elapsed));
            frames = 0; windowStart = ts; worstMs = 0;
            resetLoafWindow();
        }
    }
    rafId = requestAnimationFrame(tick);
}

function startFps(): void {
    cancelAnimationFrame(rafId);
    lastTs = 0;
    resetLoafWindow();
    lastScrollProbe = { top: -1, el: '' };
    ensureLoafObserver();
    fetchGpuStatus(); // 每次开启时取样一次（GPU 状态变更需重启才生效，不必轮询）
    ensureBadge();
    rafId = requestAnimationFrame(tick);
}

function stopFps(): void {
    cancelAnimationFrame(rafId);
    if (loafObserver) { try { loafObserver.disconnect(); } catch { /* ignore */ } loafObserver = null; }
    if (badge) { badge.remove(); badge = null; }
}

// ── 轻提示 ──
function flashHint(text: string): void {
    try {
        const old = document.getElementById(HINT_ID);
        if (old) old.remove();
        const el = document.createElement('div');
        el.id = HINT_ID;
        el.setAttribute('data-fnos-ui', '1');
        el.style.cssText = 'position:fixed;left:50%;bottom:64px;transform:translateX(-50%);z-index:2147483647;'
            + 'pointer-events:none;background:rgba(10,12,18,.85);color:#EAEFF7;font:500 13px/1.4 system-ui,sans-serif;'
            + 'padding:8px 14px;border-radius:10px;';
        el.textContent = text;
        document.body.appendChild(el);
        window.setTimeout(() => el.remove(), 2200);
    } catch { /* ignore */ }
}

// ── 注册 ──
registerHook(HookType.OnReady, () => {
    applyNoGlass(); // 模块级注入时 head 可能未就绪，此处兜底

    document.addEventListener('keydown', (e: KeyboardEvent) => {
        if (!e.ctrlKey || !e.altKey || !e.shiftKey || e.repeat) return;
        const t = e.target as HTMLElement | null;
        if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return; // 避开 AltGr 输入
        const k = (e.key || '').toLowerCase();
        if (k === 'f') {
            e.preventDefault();
            e.stopPropagation();
            const on = !lsOn(FPS_KEY);
            lsSet(FPS_KEY, on);
            if (on) startFps(); else stopFps();
            flashHint(on ? 'FPS 显示已开启 — Ctrl+Shift+Alt+F 关闭' : 'FPS 显示已关闭');
        } else if (k === 'b') {
            e.preventDefault();
            e.stopPropagation();
            const on = !lsOn(NOGLASS_KEY);
            lsSet(NOGLASS_KEY, on);
            applyNoGlass();
            flashHint(on ? '玻璃滤镜已关闭(重启仍生效) — Ctrl+Shift+Alt+B 恢复' : '玻璃滤镜已恢复');
        }
    }, true);

    if (lsOn(FPS_KEY)) startFps();
    logger.info('[perfDiag] 已加载 — Ctrl+Shift+Alt+F=FPS帧诊断表 Ctrl+Shift+Alt+B=玻璃开关');
});

// 模块级：玻璃关闭态注入（放在 registerHook 之后，遵循模块级铁律的注册优先原则）
applyNoGlass();

export { };
