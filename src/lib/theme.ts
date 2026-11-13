/* [liquid-theme] 主题作用域的**唯一**读写实现。
 *
 * 为什么不再用 `html.dark` class：
 *   液体揭示要在**同一屏**里同时呈现两套主题（底层旧 + 揭示层新）。class 是全局单例，
 *   只能有一个；`data-theme` 是属性，可以挂在任意子树（html 一份、.lt-host 一份）。
 *   CSS 侧已全部改成 `[data-theme="dark"]` / `[data-theme="light"]`（见 styles/*.css）。
 *
 * 首屏恢复：在 React 挂载前由 main.tsx 调用 applyThemeToDocument，避免闪错主题。
 */

export type DocTheme = "light" | "dark";

export const THEME_STORAGE_KEY = "fntv-theme";

/** 读持久化主题（默认浅色，与改造前行为一致） */
export function readStoredTheme(): DocTheme {
  try {
    return localStorage.getItem(THEME_STORAGE_KEY) === "dark" ? "dark" : "light";
  } catch {
    return "light";
  }
}

/** 写到 <html>：同时给 data-theme 与 color-scheme（后者影响原生滚动条/表单控件的深浅） */
export function applyThemeToDocument(theme: DocTheme): void {
  const el = document.documentElement;
  el.setAttribute("data-theme", theme);
  el.style.colorScheme = theme;
}

/** 当前生效主题（DOM 为准，而不是 React state） */
export function currentDocTheme(): DocTheme {
  return document.documentElement.getAttribute("data-theme") === "dark" ? "dark" : "light";
}
