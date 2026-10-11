# mpv-Yaozhi 功能引入可行性评估

> 2026-10-11 · 基于整合包 1.0.6-2 完整脚本盘点（69 个脚本/模块/资产组），对照 Fntv-Plus 现状
> 许可前提：Yaozhi 自创内容为 **MIT**（含我们已用的弹幕补丁），第三方脚本保留各自许可（引入时需保留头部声明）

我们的现状（`portable_config/scripts/`）：uosc、uosc_danmaku（弹幕）、smart_skip（片头片尾跳过）、
thumbfast（进度条缩略图）、fntv_interp（插帧切换）、fntv_shader（Anime4K）、fntv_nas_sub（NAS 字幕）、
fntv_replay。核心：自编译 0.41.0-1011（已含 secondary-ass 弹幕补丁系列）。

---

## 一、可直接落地（纯 Lua、零外部依赖、标准 mpv API）

| 功能 | 他们的实现 | 体积 | 说明 | 评估 |
| --- | --- | --- | --- | --- |
| **起播格式徽章** | `startup-format-logos.lua` + `script-modules/media-format-info.lua` + `startup-logo-bounds.lua` + **696 个自绘徽章 BGRA 资产** | 50+17+6KB | 播放起播时在画面安全区显示 Dolby Vision/HDR10+/Atmos/FLAC 等格式徽章，几秒后淡出。**只用标准属性**（video-params/track-list/video-frame-info），不依赖其私有核心补丁（DV Profile 用户数据缺失时优雅降级） | ★★★ 视觉辨识度最高的功能，可直接移植；需适配我们的窗口布局与禁用时机 |
| **PGS 字幕亮度** | `image-subs-brightness.lua` | 8KB | 图形字幕（PGS/VobSub）独立亮度 150-400nits 调节，动画发烧刚需 | ★★★ 纯 Lua + shader，直接可用 |
| **记住运行时属性** | `persist_properties.lua` | 3KB | 音量/倍速/字幕延迟/顶置等跨会话记忆 | ★★★ 3KB，零风险 |
| **动态去黑边** | `dynamic-crop.lua` | 40KB | 实时检测并裁掉视频黑边 | ★★ |
| **音乐模式** | `music-mode.lua` | 12KB | 只听音频时关闭视频渲染省电 | ★★ |
| **按住快进** | `evafast.lua` | 12KB | 长按右键/倍速键平滑加速（社区名脚本） | ★★ 操作手感 |
| **拖动反馈** | `seek-feedback.lua` | 6KB | seek 时的视觉反馈动效 | ★★ |
| **章节跳过** | `chapterskip.lua` | 38KB | 按章节名（OP/ED/Recap/广告）自动跳过，中英文模式表。与我们 smart_skip（theintro 数据库+手动标记）**互补不冲突**：有章节的片子走 chapterskip，没章节的走 smart_skip | ★★ |
| **快捷键管理** | `keybinding-manager.lua` | 33KB | uosc 内中文搜索/修改/恢复快捷键，写 `yaozhi-keybindings.json` 不动 input.conf | ★★ 需要少量中文化与 uosc 菜单对接 |
| 小工具集 | `ontop_mode`(置顶)、`auto-fullscreen`、`minimize-pause-resume`(最小化暂停恢复)、`pip`(画中画)、`screenshot_desktop`、`mpv-cropscreen`(截屏裁剪)、`cycle-commands`、`fix-avsync` | 各 3-15KB | 播放器易用性补全 | ★ 按需挑 |

## 二、契合场景但需适配/决策

| 功能 | 他们的实现 | 依赖 | 说明 |
| --- | --- | --- | --- |
| **播放历史 / 书签 / 最近播放** | `simplehistory`(99KB)/`simplebookmark`(131KB)/`recentmenu`(18KB)/`history-bookmark` | 纯 Lua | Eisa AlAwadhi 的第三方脚本（保留其许可）。**需先确认与 App 侧「继续观看」是否重复**——NAS 播放的进度我们 App 已管；对本地/网盘直链播放是补充 |
| **ASSRT 字幕搜索下载** | `sub-assrt.lua` | API token（assrt.net 注册） | NAS 片源缺字幕时在播放器内直接搜字幕；中文字幕站，契合我们场景。需 App 设置面板加 token 配置项 |
| **字幕自动对轴** | `autosubsync/` | 待核实（含外部二进制则重） | 外挂字幕音画对轴；落地前要查它的运行时 |
| **字幕导出 / 字幕行增强** | `sub_export.lua`、`uosc-subtitle-lines.lua` | 纯 Lua | 低成本补齐 |
| **SponsorBlock** | `sponsorblock_minimal.lua` | 网络 API | 仅 YouTube 场景有意义，随在线媒体一起决策 |

## 三、大型功能（需立项，含运行时/大量资产/多模块联动）

| 功能 | 构成 | 工作量判断 |
| --- | --- | --- |
| **在线媒体（B站/YouTube 解析、直播播放+弹幕）** | `online-media.lua` + `online-media-router` + `network-backend` + **自带 Python 运行时**（yt-dlp 等）+ `bilibili/douyu/youtube-account.lua`（cookie）+ uosc_danmaku 的 `sites/`（直播弹幕） | 重。**更合理的路线：解析放我们 Electron 侧**（已有 B站弹幕链路 biliRunner），mpv 只收直链；直播弹幕可从他们的 `uosc_danmaku/sites/` 移植 |
| **AI 补帧 RIFE** | `ai-interpolation.lua` + `ai-background.lua` + `adaptive-quality.lua` + RIFE Vulkan 模型（需另下）+ TensorRT 可选 | 重。我们的 fntv_interp 是 tscale 级插帧切换；RIFE 是另一档（需模型文件与 GPU 要求），且他们有「智能画质保护」联动。立项前先确认用户显卡满足 |
| **蓝光/DVD 原盘菜单** | `disc-menu.lua` + `bluray-titles-menu.lua` + 核心补丁 0004-0015 + **捆绑 JRE**（350MB） | NAS 流媒体场景价值低、体积大，不建议 |
| **HDR/DV/AV3A/Atmos 深度支持** | 核心补丁（luna 16 连 + DV 系列 + AV3A ffmpeg 补丁）+ `hdr-mode` 资产 + `audio-passthrough.lua` | 我们的构建管线已具备打补丁能力，但用户群（动漫流媒体）收益低；等真实需求 |
| **文件浏览器 / ISO 加载 / ZIP-RAR 播放** | `file-browser/`（多模块+addons）+ `auto_iso_loader` | 我们的播放入口在 App 侧，mpv 内再浏览文件场景少 |

## 四、核心补丁类可选项（我们已有构建管线，按需加）

他们的顶层 `patches/0001-0016`（MIT）独立可用：
- `0001/0008 vo_gpu_next 图像字幕色彩控制`（PGS 在 HDR 屏上的观感）——与 image-subs-brightness 配套
- `0002/0003/0005/0006 WASAPI/SDL 空间声道保留`——多声道用户
- `0004 HDR Vivid 元数据暴露`——配合徽章显示
- `0010-0014 demux 非活动字幕缓存`——多字幕轨加载提速

## 五、建议的首批引入清单（性价比排序）

1. **persist_properties**（3KB，立即生效）
2. **PGS 字幕亮度**（8KB，动漫刚需）
3. **起播格式徽章**（含资产整体移植，我们的招牌视觉功能候选）
4. **chapterskip + seek-feedback + evafast**（播放手感三件套）
5. **dynamic-crop + music-mode**（按需）
6. **sub-assrt 字幕搜索**（需 App 加 token 配置）

预估：1-4 项合计约一天工作量（含 uosc 菜单入口对接、选项写入我们的 mpvConfig 面板、中文文案统一）。

## 附：盘点原始清单

他们的 69 项中与我们完全重叠/不需要的：uosc（有）、uosc_danmaku（有，我们的版本特性更多）、
thumbfast（有）、skip-segments（我们叫 smart_skip）、stats、autoload、品牌类（idle-branding/yaozhi-home/
yaozhi-donation）、账号类（bilibili/douyu/youtube-account，随在线媒体）、steam 双开器类（startup-window、
mpv-animated 窗口动画可选）。
