/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { AppSettings } from "../types";

export const DEFAULT_SETTINGS: AppSettings = {
  exitMode: "ask",
  perfMode: false,
  carouselLogo: true,
  launchPage: "home",
  autoLogin: false,
};
/** 平均亮度阈值（0..255）：> 阈值 = 亮底 → 深色 chrome */
