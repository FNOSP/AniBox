/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { AppSettings } from "../types";
import { DEFAULT_LIQUID_AUDIO_PREFS, loadLiquidAudioPrefs, saveLiquidAudioPrefs } from "../components/liquid-theme/prefs";

export const DEFAULT_SETTINGS: AppSettings = {
  exitMode: "ask",
  perfMode: false,
  carouselLogo: true,
  launchPage: "home",
  autoLogin: false,
  // [lc-1561] 自建弹幕接口默认开启：地址留空即用随包内置源（本地 danmu_api）
  danmuApiEnabled: true,
  // [lc-1753] 在线资源搜索默认关闭：需在设置面板知悉「不存储不分发资源」声明后手动开启
  webrsPanelEnabled: false,
  danmuApiBases: [],
  danmuApiBase: "",
  // [lc-2056] 兜底阈值默认 0 = 不启用：主源完全没有弹幕时才走弹弹play（原流程）
  danmuDdFallbackBelow: 0,
  // 弹幕引擎默认 node = 与接入前行为完全一致（Rust 侧 `danmu_engine` 默认同为 "node"）
  danmuEngine: "node",
  // [lc-1909] 播放打开方式：默认软件内播放（与 Rust 侧 player_open_mode 默认同口径）
  playerOpenMode: "window",
  // [lc-2057] 播放时自动隐藏主窗：默认关闭（保持原行为）
  hideMainOnPlay: false,
  // [lc-2048] 默认液态玻璃（与 Rust 侧 default_player_skin 同口径）
  playerSkin: "glass",
  // [lc-2075] 玻璃模糊默认开（与 Rust 侧 player_glass_blur 同口径）
  playerGlassBlur: true,
  // 界面不透明度（百分比）：默认 85，范围 60–100（Rust 侧 set_settings 同口径 clamp）
  uiOpacity: 85,
  // [liquid-theme] 主题切换音效默认关闭（需求明确要求）；音量 45%
  themeSound: DEFAULT_LIQUID_AUDIO_PREFS.enabled,
  themeSoundVolume: DEFAULT_LIQUID_AUDIO_PREFS.volume,
};

/* [liquid-theme] 主题音效是**纯前端**偏好：Rust 的 set_settings 白名单里没有这两键，
   走 IPC 会被回滚成旧值。所以单独存 localStorage（模块 prefs.ts），
   并在 App 启动时用它覆盖镜像里的默认值。 */
export function readThemeSoundPrefs(): { themeSound: boolean; themeSoundVolume: number } {
  const p = loadLiquidAudioPrefs();
  return { themeSound: p.enabled, themeSoundVolume: p.volume };
}

export function writeThemeSoundPrefs(patch: { themeSound?: boolean; themeSoundVolume?: number }): void {
  const cur = loadLiquidAudioPrefs();
  saveLiquidAudioPrefs({
    enabled: patch.themeSound ?? cur.enabled,
    volume: patch.themeSoundVolume ?? cur.volume,
  });
}
/** 平均亮度阈值（0..255）：> 阈值 = 亮底 → 深色 chrome */
