/* [lc-2064] 版本更新说明弹窗：**每个版本只弹一次**（不论热补丁还是安装包大更新）。
 *
 * 与 UpdatePrompt 的分工（别混）：
 *   · UpdatePrompt（lc-1710）= 「有新版本可用，要不要装」—— 依赖远端 update_check，
 *     只在真的检测到新版本时出现；
 *   · 本组件 = 「你当前这一版改了什么」—— 纯本地行为，装上/热更之后打开软件就能看到，
 *     不需要联网、也不关心是怎么更新上来的。
 *
 * 版本号**不写死在任何地方**：运行时由 `getVersion()`（Tauri，读 tauri.conf.json，
 * 也就是安装包真实版本）给出，本模块只负责「拿这个号去查说明表」。发版时唯一要做的
 * 是往 NOTES 里加一条对应版本号的条目 —— 版本号本身只改 package.json /
 * tauri.conf.json / Cargo.toml 三处（本来就该同版本）。
 *
 * 只弹一次的实现：localStorage `anibox.seenChangelog.<version>` 记录版本号；
 * 清缓存/换设备会重弹一次，这是可接受的（没有服务端可存）。
 */

export type ChangelogEntry = {
  /** 标题（一句话点题） */
  title: string;
  /** 要点：一句一条，保持简短 */
  items: string[];
  /** 提示条：需要用户注意的操作（可选） */
  tip?: string;
  /** 测试功能提醒（可选）：渲染成一条显眼的「测试中」提示，引导反馈 */
  beta?: string;
};

/**
 * 说明表：**key = 版本号**（与 tauri.conf.json 的 version 完全一致，不带 v 前缀）。
 * 新版本往最前面加一条即可，不需要改组件、不需要改任何「当前版本」常量。
 *
 * 跨多个版本升级的用户只会看到**当前版本**这一条（不做版本区间合并）——
 * 弹窗定位是「这版改了什么」，不是完整更新日志；历史条目留给回退/排查时查。
 */
export const NOTES: Record<string, ChangelogEntry> = {
  "4.2.0": {
    title: "播放器重做：液态玻璃皮肤与画质引擎",
    items: [
      "液态玻璃皮肤：底栏、弹窗、右键菜单整体重绘，半透明磨砂质感，带缩略图预览与新的快捷入口。",
      "左上角状态条：插帧、置顶、着色器、弹幕数据各占一条，从起播信息变成一眼可读的实时状态。",
      "着色器优化：内置 50 档预设，覆盖动漫线条重建（Anime4K）、真人放大（FSRCNNX / ArtCNN / CuNNy）与降噪，按片源一键切换。",
      "插帧方案接入：内置平滑、RIFE AI 2×、SVP 60FPS、MVTools 2× 四档，跟不上实时的会自动降级保音画同步。",
      "JavHub 与多云盘接入为测试功能：新增番号识别库、在线刮削与手机网盘直连。",
    ],
    beta: "JavHub 与云盘接入仍在测试中，遇到问题欢迎在「设置 → 反馈」里提给我们。",
    tip: "不喜欢新皮肤？到 设置 → 播放器 → 播放器皮肤 切回「原生（uosc）」。",
  },
};

/** 去掉版本号里的 v 前缀并 trim（后端/构建产物可能带 v）。 */
export function normalizeVersion(v: string): string {
  return (v || "").trim().replace(/^v/i, "");
}

/** 该版本是否已经弹过（true = 已看过，不用再弹）。 */
export function changelogSeen(version: string): boolean {
  const v = normalizeVersion(version);
  if (!v) return true;
  try {
    return localStorage.getItem(`anibox.seenChangelog.${v}`) === "1";
  } catch {
    // localStorage 不可用（隐私模式等）→ 视为已看过，避免每次启动都弹
    return true;
  }
}

/** 标记该版本说明已展示。 */
export function markChangelogSeen(version: string): void {
  const v = normalizeVersion(version);
  if (!v) return;
  try {
    localStorage.setItem(`anibox.seenChangelog.${v}`, "1");
  } catch {
    /* ignore */
  }
}

/** 取指定版本对应的说明条目（没有就返回 null，不弹）。 */
export function changelogFor(version: string): (ChangelogEntry & { version: string }) | null {
  const v = normalizeVersion(version);
  if (!v) return null;
  const e = NOTES[v];
  return e ? { version: v, ...e } : null;
}
