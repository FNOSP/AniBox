import { registerHook, HookType } from '../core/hooks';

// playMemory.ts — [lc-1065] 播放参数记忆（对标 Infuse/Netflix：倍速/音量/静音跨集记忆）
// ─────────────────────────────────────────────────────────────────────────────
// 网页播放器换集后 倍速/音量 会被重置为默认(1x/100%)。本插件：
//   · 监听 video 的 ratechange/volumechange → 持久化到 localStorage（全局记忆）；
//   · 新视频加载(loadedmetadata/play)时自动恢复上次参数；
//   · 恢复窗口期(播放起 3s 内)对抗播放器初始化覆盖：检测到被重置立即打回（不落盘），
//     窗口期后视为用户真实操作照常持久化。
// 作用域：video 元素级直接读写 playbackRate/volume/muted，不依赖播放器内部 API，
//   网页播放器/任意 <video> 均生效。MPV 端由 watch-later/自身配置管理，不在此列。
//
// [lc-2323] **静音不做记忆**（原实现会把「播放器自己静音」记成用户意图，导致永久无声）：
//   实测（内置浏览器 + 真 NAS 复现）飞牛网页播放器 xgplayer 会在两处自行写 `media.muted=true`：
//     ① 播放器 destroy()：`media.pause(); media.muted = true;`（换集/离开播放页都会走）；
//     ② 浏览器允许静音自动播放时它按 muted 起播兜底（用户从未点过静音）。
//   旧实现在 volumechange 里无条件 `setItem(LS_MUTE, muted?'1':'0')`，上述两次都被记成
//   「用户想静音」→ 下次起播 restore() 把 muted 强行打回 true → 画面正常但完全没声音，
//   而且此后每次播放都被打回（用户侧表现：这台机器 Edge 打开就是无声，换新浏览器又好了）。
//   现在只记忆音量与倍速：`muted` 一律不持久化，也不在 restore 里改写 —— 用户真静音由
//   播放器自己的 UI 状态承担（同一次会话内有效），跨会话静音不记忆是更安全的口径。
// 字幕轨/音轨记忆涉及播放器内部轨道状态，后续版本单独接入。

const LS_RATE = 'fntv-play-rate';
const LS_VOL = 'fntv-play-volume';
const LS_MUTE_LEGACY = 'fntv-play-mute'; // 历史键：只做清理，不再读写
const GUARD_MS = 3000; // 恢复窗口期：播放起 3s 内视为"播放器初始化覆盖"对抗期

const num = (key: string, fallback: number): number => {
    try {
        const v = parseFloat(localStorage.getItem(key) || '');
        return Number.isFinite(v) ? v : fallback;
    } catch { return fallback; }
};

/** [lc-2323] 清掉历史遗留的静音记忆键：它一旦为 '1' 就会让播放器永久静音。 */
function purgeLegacyMuteKey(): void {
    try { localStorage.removeItem(LS_MUTE_LEGACY); } catch { /* ignore */ }
}

function attach(v: HTMLVideoElement): void {
    if (v.dataset.fntvPlayMem === '1') return;
    v.dataset.fntvPlayMem = '1';

    let suppressUntil = 0; // 恢复对抗期截止

    // ── 持久化（只存倍速与音量；静音不落盘，理由见文件头）──
    v.addEventListener('ratechange', () => {
        const saved = num(LS_RATE, 1);
        if (Date.now() < suppressUntil && saved > 0 && Math.abs(v.playbackRate - saved) > 0.01) {
            v.playbackRate = saved; // 播放器初始化重置 → 打回
            return;
        }
        try { localStorage.setItem(LS_RATE, String(v.playbackRate)); } catch { /* ignore */ }
    });
    v.addEventListener('volumechange', () => {
        // 不处理 muted 变化：那是播放器自身行为（销毁/静音起播），不是用户意图。
        if (v.muted) return;
        const savedVol = num(LS_VOL, -1);
        if (Date.now() < suppressUntil) {
            const volOff = savedVol >= 0 && Math.abs(v.volume - savedVol) > 0.01;
            if (volOff) {
                v.volume = savedVol;
                return;
            }
        }
        try { localStorage.setItem(LS_VOL, String(v.volume)); } catch { /* ignore */ }
    });

    // ── 恢复 ──
    const restore = (): void => {
        const savedRate = num(LS_RATE, 0);
        const savedVol = num(LS_VOL, -1);
        purgeLegacyMuteKey();
        if (savedRate > 0 && Math.abs(v.playbackRate - savedRate) > 0.01) v.playbackRate = savedRate;
        if (savedVol >= 0) v.volume = savedVol;
        // [lc-2323] 绝不在这里写 v.muted：曾把播放器静音起播固化成"用户静音"。
        // 播放器若以静音起播（浏览器自动播放策略兜底），交给用户点音量图标解除。
        suppressUntil = Date.now() + GUARD_MS;
        // 短周期重申：播放器初始化晚于我们覆盖时打回
        let n = 0;
        const reassert = window.setInterval(() => {
            n++;
            if (n >= 6 || !v.isConnected) { clearInterval(reassert); return; }
            const r = num(LS_RATE, 0);
            if (r > 0 && Math.abs(v.playbackRate - r) > 0.01) v.playbackRate = r;
        }, 500);
    };

    v.addEventListener('loadedmetadata', restore);
    v.addEventListener('play', restore);
    if (v.readyState >= 1) restore();
    // 事件早于本插件挂载的兜底：延迟补恢复一次（真实场景: 换集时 loadedmetadata 常先于轮询挂载）
    window.setTimeout(restore, 600);
}

// ── 轮询挂载：SPA 换集可能替换 video 元素 ──
registerHook(HookType.OnReady, () => {
    purgeLegacyMuteKey(); // 插件加载即清理，避免旧键在首次 restore 之前就产生影响
    window.setInterval(() => {
        try {
            const v = document.querySelector('video') as HTMLVideoElement | null;
            if (v) attach(v);
        } catch { /* ignore */ }
    }, 1500);
});
