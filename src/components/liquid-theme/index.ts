/* [liquid-theme] 模块出口。
 *
 * 最小用法（见 README 的完整示例）：
 *   const { toggle } = useLiquidThemeReveal({
 *     theme, onChange: setTheme,
 *     applyDomTheme: (t) => document.documentElement.setAttribute("data-theme", t),
 *     audioEnabled, audioVolume,
 *   });
 *   <button onClick={(e) => toggle(e)}>切换</button>
 */

// 样式随模块走：调用方只需 import 本模块，无需再手动引 CSS。
// （不能放进 src/styles/index.css 的 @import 链 —— CSS 规定 @import 必须在所有规则之前，
//   混在长链中间会被静默丢弃；实测服务端 CSS 里 lt-stage 出现 0 次。）
import "./liquid-theme.css";

export { createLiquidReveal, supportsDynamicClipPath } from "./reveal.ts";
export type { LiquidRevealHandle, LiquidRevealDeps, CreatedLiquidReveal } from "./reveal.ts";
export { buildLiquidShape } from "./geometry.ts";
export type { LiquidGeoInput, LiquidShape } from "./geometry.ts";
export { createLiquidAudio, auditionLiquidStage } from "./audio.ts";
export type { LiquidAudio, LiquidAudioOptions } from "./audio.ts";
export { useLiquidThemeReveal, resolveOrigin, DEFAULT_LIQUID_DURATION, LIQUID_THEME_ATTR } from "./useLiquidThemeReveal.ts";
export type { LiquidThemeApi, LiquidThemeHookOptions } from "./useLiquidThemeReveal.ts";
export {
  STAGE_BOUNDS,
  STAGE_START,
  stageAt,
  stageProgress,
  directionOf,
  directionPitch,
} from "./timeline.ts";
export { LIQUID_PALETTE, LIQUID_BACKING, LIQUID_SHEEN } from "./palette.ts";
export { loadLiquidAudioPrefs, saveLiquidAudioPrefs, DEFAULT_LIQUID_AUDIO_PREFS } from "./prefs.ts";
export type { LiquidAudioPrefs } from "./prefs.ts";
export type {
  LiquidTheme,
  LiquidDirection,
  LiquidStage,
  LiquidFrameSnapshot,
  LiquidAudioAdapter,
  LiquidOrigin,
  LiquidOriginSource,
  LiquidRevealConfig,
  UseLiquidThemeOptions,
} from "./types.ts";
