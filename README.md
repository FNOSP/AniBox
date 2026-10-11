# Fntv-Plus Tauri2 (PoC)

Fntv-Plus 的 Tauri 2 迁移概念验证版：**告别网页注入，自绘 UI + 直调飞牛影视 API + mpv 双层窗口内嵌**。

背景与方案文档见主仓库 `resource/wiki/Tauri2迁移方案.md`。

## 运行

```bat
npm install
npm run tauri dev
```

首次 `tauri dev` 会编译 Rust（约 5-15 分钟），之后增量秒级。

## 登录页配置

- **服务器地址**：内网 `http://IP:18888`（默认端口 18888）
- **高级设置**：可改 mpv.exe / Go 代理路径，默认复用主仓库：
  - `D:/GitHub/Fntv-Plus/third_party/fntv-mpv/mpv.exe`
  - `D:/GitHub/Fntv-Plus/third_party/proxy/proxy.exe`
- 配置落盘：`%APPDATA%/com.fntv.plus.tauri2/config.json`

## 架构（PoC 范围）

| 层 | 实现 |
| --- | --- |
| UI | React 19 + 自绘（登录页/媒体库网格/详情二级页/播放页），DWM acrylic 玻璃壳 |
| API | Rust reqwest 直调 `/v/api/v1/*`，Authx 签名逐字节对齐主仓库 `request.ts`（含 POST nonce、5000 重试、3xx 跟随、自签证书放行） |
| 播放 | **双层窗口**：mpv.exe `--wid` 渲染进主窗口内的 Win32 子窗口（`SetWindowPos(HWND_BOTTOM)` 压在透明 WebView 之下），前端占位 div 用 ResizeObserver 同步物理像素 bounds；控制栏为纯 HTML，事件经 Tauri command → JSON IPC（命名管道）转 mpv |
| 直链 | 复用 Go 代理（22346 端口守护 + play_url 参数与 media.ts getProxyUrl 逐段对齐） |

### PoC 未包含（按方案属阶段 2+）

弹幕数据层、FN ID 登录、Trakt/豆瓣、进度上报、设置面板写 mpv conf、smart_skip 等脚本资产挂载（当前 mpv 以 `--no-config` 最小参数启动）。

## 已知边界

- mpv 进程与主窗口同生命周期（退出时清场）；Go 代理孤儿进程未做清理（主仓库 proxy.ts 的 killOrphan 逻辑待移植）
- Win10 落到非亚克力外观（acrylic 需 Win11 22H2+，未做门控降级）
- 键盘快捷键未接管（mpv 侧已禁默认键位，控制全走 UI）
