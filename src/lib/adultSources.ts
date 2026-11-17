/* [lc-2217] 短剧「18+ 站源」解锁态（默认关）。
   口径（用户明确要求）：站源里**只有红果**是公开内容；其余（剧果 / 禁果短剧 / 野果 / 黑桃 /
   黄豆 / 青空 …）一律按 18+ 处理 —— 默认不出现，必须在 设置 → 通用 → 短剧 18+ 站源
   手动打开，且打开时要**手动输入确认语**（`ADULT_SOURCES_PHRASE`）才生效。

   实现：localStorage 存开关 + 自定义事件广播。设置面板改完，发现页的站源 chips 立刻跟着变
   （DuanjuPane 订阅），不用等页面重挂载。 */

/** 开启 18+ 站源时必须一字不差输入的确认语（设置页二次确认浮层里展示给用户） */
export const ADULT_SOURCES_PHRASE = "我已满18";

const KEY = "anibox-adult-sources-v1";
const EVT = "anibox:adult-sources";

export function adultSourcesEnabled(): boolean {
  try {
    return localStorage.getItem(KEY) === "1";
  } catch {
    // localStorage 不可用（隐私模式）：按关闭处理，靠本次会话的事件生效
    return false;
  }
}

export function setAdultSourcesEnabled(on: boolean) {
  try {
    if (on) localStorage.setItem(KEY, "1");
    else localStorage.removeItem(KEY);
  } catch {
    /* 存储被禁：至少让本次会话的订阅方收到变化 */
  }
  try {
    window.dispatchEvent(new Event(EVT));
  } catch {
    /* ignore */
  }
}

/** 订阅开关变化（返回取消函数）。额外监听 storage，覆盖多窗口 / 外部改动。 */
export function onAdultSourcesChange(cb: () => void): () => void {
  window.addEventListener(EVT, cb);
  window.addEventListener("storage", cb);
  return () => {
    window.removeEventListener(EVT, cb);
    window.removeEventListener("storage", cb);
  };
}

/** 站源是否属于 18+：红果之外一律算（按用户口径，不看核心回传的 adult 声明） */
export function isAdultSource(id: string): boolean {
  return id !== "hongguo";
}
