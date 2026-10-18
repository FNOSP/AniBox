/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { AppSettings } from "../types";

export const DEFAULT_SETTINGS: AppSettings = {
  exitMode: "ask",
  perfMode: false,
  carouselLogo: true,
  launchPage: "home",
  autoLogin: false,
  // [lc-1561] 自建弹幕接口默认开启：地址留空即用随包内置源（本地 danmu_api）
  danmuApiEnabled: true,
  danmuApiBases: [],
  danmuApiBase: "",
  // 弹幕引擎默认 node = 与接入前行为完全一致（Rust 侧 `danmu_engine` 默认同为 "node"）
  danmuEngine: "node",
};
/** 平均亮度阈值（0..255）：> 阈值 = 亮底 → 深色 chrome */
