import React from "react";
import ReactDOM from "react-dom/client";
import { Root } from "./App";
import { applyThemeToDocument, readStoredTheme } from "./lib/theme";

// [liquid-theme] 首屏主题在 React 挂载前落到 <html data-theme>，避免刷新时闪现错误主题。
// 这里**同步**读 localStorage：晚一帧就会看到浅色一闪。
applyThemeToDocument(readStoredTheme());

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>,
);
