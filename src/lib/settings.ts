/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { AppSettings } from "../types";

export const DEFAULT_SETTINGS: AppSettings = {
  exitMode: "ask",
  perfMode: false,
  carouselLogo: true,
  launchPage: "home",
  autoLogin: false,
  // [lc-1561] 自建弹幕接口默认关闭 = 完全走内置 B站 链路（与接入前行为一致，对齐主仓库默认）
  danmuApiEnabled: false,
  danmuApiBases: [],
  danmuApiBase: "",
};
/** 平均亮度阈值（0..255）：> 阈值 = 亮底 → 深色 chrome */
