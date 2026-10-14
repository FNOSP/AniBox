# AniBox（番盒）

飞牛影视（fnOS）桌面客户端：**告别网页注入，自绘 UI + 直调飞牛影视 API + mpv 原生子窗嵌入**。

背景与方案文档见主仓库 `resource/wiki/Tauri2迁移方案.md`。

## 运行

```bat
dev.cmd
```

`dev.cmd` 会：杀残留进程 → 释放 vite 端口 1420（**注意：端口被占时 Windows 返回的是 EACCES 而不是
EADDRINUSE**，且本机动态端口范围从 1024 起，任何进程的出站连接都可能临时抢走 1420 —— 清理逻辑与
处置建议都写在脚本注释里）→ `tauri dev`。首次编译约 5-15 分钟，之后增量秒级。

## 登录页配置

- **服务器地址**：内网 `http://IP:18888`（默认端口 18888）
- **高级设置**：可改 mpv.exe / Go 代理路径，默认复用主仓库：
  - `D:/GitHub/Fntv-Plus/third_party/fntv-mpv/mpv.exe`
  - `D:/GitHub/Fntv-Plus/third_party/proxy/proxy.exe`
- 配置落盘：`%APPDATA%/com.anibox.app/config.json`
  （旧版 `com.fntv.plus.tauri2` 的数据会在首次启动时**自动迁移**，含缓存与 WebView 配置）

---

## 播放器架构（本项目的核心，改之前务必读完）

### 窗口拓扑

```
Tauri 主窗口        decorations:false / transparent:true / DWM 圆角 + 亚克力底板
├─ WebView2 各层窗口  WRY_WEBVIEW / Chrome_WidgetWin_0|1 / Intermediate D3D Window /
│                     Chrome_RenderWidgetHostHWND   ← 全部被 SetWindowRgn 挖掉「视频矩形」
└─ AniBoxMpvHost       WS_CHILD · HWND_BOTTOM · 纯黑底色   ← 洞口透出来的就是它
   └─ mpv 自建子窗    class "mpv"（mpv.exe 进程内建；被 mpv 自己禁用 → 我们 EnableWindow 打开）
```

### 三层职责（几何只有一个来源）

| 组件 | 职责 |
| --- | --- |
| 前端 `App.tsx` | 播放页存续期间 **rAF 逐帧**测出视频占位矩形（物理像素）→ `player_set_bounds`。逐帧对比是为了一次性消灭"事件猜谜"式同步的遗漏与滞后 |
| `lib.rs::player_rect()` | **唯一来源**：全屏态 → 主窗客户区；否则用前端值（退出全屏那一拍前端还没重传，用 `last_embedded` 兜底）。**「洞」和「宿主窗」都取它** ⇒ 两者永不不一致 |
| `win_region.rs` | 枚举 WebView2 各层窗口，把「客户区 − 洞」用 `CombineRgn(RGN_DIFF)` 合成后 `SetWindowRgn`。洞带圆角；宿主窗另有 3px overscan 盖住对齐缝隙 |

### 为什么必须是这套（关键决策，别再退回"把视频画进网页"）

- **mpv 的画面只能以原生窗口呈现。** WebView2 不提供共享 GPU 纹理接口；软件路径
  （render API → 每帧位图 → canvas）在 1080p60 下约 500MB/s 拷贝，直接卡死。
- 参考实现 **LumiPlayer 用同一套**（`LumiPlayerNativeVideoHost` + `CreateWindowExW(WS_CHILD,"STATIC")`
  + `SetWindowRgn` 挖洞），它也不是"像素级内嵌"。
- **挖洞（而不是把视频窗置顶）**的意义：WebView 保持"逻辑最上层"，页面 HTML 能正常盖住视频区
  之外的一切；并且洞对鼠标也是透传的 ⇒ 视频区内的鼠标归 mpv，**原生 uosc 底栏 / 右键菜单 / 双击全屏
  才能用**。代价：视频区内的像素归 mpv，页面 HTML 盖不到（所以底栏交给 uosc，而不是自绘 HTML 条）。

### mpv 侧与三个必知坑

独立 `mpv.exe` 进程 + `--wid=<AniBoxMpvHost>` + `--input-ipc-server=\\.\pipe\anibox-mpv-<pid>`，
共用主仓库 `third_party/fntv-mpv/portable_config`（uosc / input.conf / 脚本 / 着色器原样生效）。

1. **mpv 嵌入时会把自建子窗 `EnableWindow(0)`**（mpv 源码 `w32_common.c:2163`，意图是"让宿主收输入"），
   而 Windows 命中测试**跳过 disabled 窗口** ⇒ 鼠标永远进不了 mpv（点击全落到我们宿主窗的
   `DefWindowProc`）→ 必须先外部 `EnableWindow(true)`。见 `win_embed::enable_mpv_child`。
2. **`--wid` 下 mpv 的 `fullscreen` 只是属性**（`update_fullscreen_state()` 对嵌入窗 early-return，
   不动窗口几何）⇒ 全屏必须由 App 把自己的窗口铺满。该属性被复用为
   **uosc 底栏双形态的判据**（`script-opts/uosc.conf` 的 `controls` 按 `fullscreen` 切两套）。
3. **尺寸一律由宿主窗决定**：mpv.conf 里的 `geometry` 之类对嵌入窗不生效/部分生效，别指望它。

### 环境开关

| 变量 | 作用 |
| --- | --- |
| `FNTV_CUTOUT=0` | 关闭挖洞，退回"视频窗置顶"形态（lc-082~087 的旧路线） |
| `FNTV_AUTOPLAY_TEST=<guid>` | 启动 5s 后走真链路自动播放（无人值守复现） |
| `FNTV_EVAL_JS=<js>` | 启动 5s 后在主 webview 执行 JS（可视验证 / 注入探针） |

### 诊断（排查播放/全屏问题先看这两个）

- `%TEMP%/anibox.log` —— 应用侧：`[win] outer/inner/scale/fullscreen/bounds`、`[cutout]` 逐窗结果、
  `CRASH` 记录。**带相对毫秒时间戳**，可直接还原"哪一拍用了旧值"。
- `%TEMP%/anibox-mpv.log` —— mpv 自身日志（`Window size` / `Video display` / 脚本加载 / 滤镜报错）。
- `.workbuddy/tools/` —— 验证脚本：`screen_shot.py`（屏幕真值抓图 + 命中测试）、`test_mpv_mouse.py`
  （鼠标输入 A/B）、`test_mpv_focus.py`（焦点）、`gen_uosc_controls.py`（uosc 底栏双形态生成）。
  注意：**合成鼠标事件进不了 WebView2**，凡依赖页面内 pointerdown 的改动无法用模拟输入验证。

## 其他层

| 层 | 实现 |
| --- | --- |
| UI | React 19 + 自绘（登录页 / 媒体库网格 / 详情二级页 / 播放页）；无边框窗口自绘标题栏 + `data-tauri-drag-region` 拖拽 + `.rsz*` 透明拖拽条缩放（tao 的无边框热区只有约 4 CSS px，太薄） |
| API | Rust reqwest 直调 `/v/api/v1/*`，Authx 签名逐字节对齐主仓库 `request.ts`（POST nonce、5000 重试、3xx 跟随、自签证书放行） |
| 直链 | 复用 Go 代理（22346 端口守护 + play_url 参数与 `media.ts` getProxyUrl 逐段对齐） |

### PoC 未包含（按方案属阶段 2+）

弹幕数据层、FN ID 登录、Trakt/豆瓣、进度上报、设置面板写 mpv conf 等。

## 已知边界

- mpv 进程与主窗口同生命周期（退出时清场）；Go 代理孤儿进程未做清理（主仓库 `proxy.ts` 的 killOrphan 待移植）
- Win10 落到非亚克力外观（acrylic 需 Win11 22H2+，未做门控降级）
- 全屏态下视频铺满整窗 ⇒ 页面 HTML 一律不可见（挖洞 = 整窗），退出全屏靠 ESC / uosc 按钮 / 双击画面
- 键盘焦点会在 mpv 子窗与页面之间切换：页面转发键（`mpv_input`）与 mpv 原生键位并存，待统一
