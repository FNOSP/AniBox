/// <reference types="vite/client" />

/* [lc-1710] dev 预览：控制台调用即可用模拟数据打开启动更新弹窗 */
interface Window {
  /** 真实更新弹窗测试：按钮会真正下载/安装，默认版本 3.9.0 */
  __aniboxRealUpdate?: (version?: string, type?: "full" | "hotfix") => void;
}