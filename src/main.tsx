import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";

// 主题在首帧渲染前落到 html 上（默认浅色），避免启动闪深色
if (localStorage.getItem("fntv-theme") === "dark") {
  document.documentElement.classList.add("dark");
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
