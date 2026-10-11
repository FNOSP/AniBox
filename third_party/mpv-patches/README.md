# Fntv-Plus mpv 核心：mpv-Yaozhi 生产栈补丁集

`third_party/fntv-mpv/mpv.exe` 由上游 mpv + 本目录补丁编译而成。

## 版本信息

- **基线**：mpv 上游 commit `c318236b8882af860f16f936225430ad053a2179`
  （即 mpv v0.41.0-1011 前的 master；mpv-Yaozhi 1.0.6-2 的核心基线，其版本号 0.41.0-1050）
- **当前栈**：c318236 + 本目录全部补丁（分支 `fntv-core-1062`，提交
  `fntv: adopt mpv-Yaozhi 1.0.6-2 production core stack`），对齐 Yaozhi 1.0.6-2 生产内核
- **补丁来源**：mpv-Yaozhi 公开仓库补丁（`build/core-migration/luna-mpv-c318236.patch` 与
  `build/bluray-menu/patches/`，作者以 git format-patch 形式公开，按上游 mpv GPLv2+ 集成）
- **历史分支**：`fntv-danmaku`（仅弹幕系列的旧栈）保留可回退

## 能力一览

| 域 | 补丁 | 作用 |
| --- | --- | --- |
| 蓝光/DVD 原盘 | 0001(luna) + 0005/0006/0007/0011/0013/0014/0024-0029 | HDMV 交互层就绪与弹出层原子叠加、蓝光菜单音频恢复/队列/诊断/采样率、标题重同步、导航头保留、DTS-HD seek 重同步、幻灯片音频 |
| 蓝光远程镜像 | 0001(luna, 含 patches/0016) | 支持可 seek 的远程蓝光镜像 |
| 画面/字幕色彩空间 | 0001(luna, 含 patches/0001) | vo_gpu_next 图像/字幕色彩空间控制 |
| HDR Vivid 识别 | 0001(luna, 含 patches/0004) | `video-params/hdr-vivid` 布尔属性（ST 2094-60 侧数据透出）—— 起播格式标签依据 |
| 空间声布局 | 0001(luna, 含 patches/0002/0003/0005-0007) | wasapi/sdl 保留高度声道与空间声布局、swresample 规范化 |
| 内封字幕缓存 | 0001(luna, 含 patches/0009-0014) | 未激活内封字幕缓存（换轨不重解） |
| DV 增强 | 0032 | DV RPU 增强元数据 |
| 弹幕渲染基石 | 0010/0015/0016/0017/0018-0021/0030/0031 | 副字幕 ASS 铺满视口、原生计时诊断、按刷新率独立重绘、`secondary-ass-replace` 原子换轨、空闲/预算/低帧率/deadline 配套 |

## 应用顺序（在干净 c318236 上，实测通过）

```
git checkout -b fntv-core-1062 c318236b8882af860f16f936225430ad053a2179
git apply 0001-luna-mpv-c318236.patch
git apply --exclude=DOCS/man/options.rst -3 0010-secondary-ass-full-viewport.patch
git apply --include=sub/sd_ass.c 0015-native-timing-diagnostics.patch
git apply 0016-secondary-ass-refresh.patch
git apply 0018-secondary-ass-idle.patch
git apply 0019-secondary-ass-video-budget.patch
git apply 0020-secondary-ass-video-headroom.patch
git apply 0021-secondary-ass-budget-stability.patch
git apply 0030-secondary-ass-low-fps-budget.patch
git apply 0031-secondary-ass-video-deadline.patch
git apply 0005-hdmv-overlay-video-ready.patch
git apply 0006-bluray-title-resync-hdmv-context.patch
git apply 0007-select-late-video-enhancement-layer.patch
git apply 0009-mkv-optional-tail-tags.patch
git apply 0011-bluray-retain-navigation-head.patch
git apply 0013-dts-hd-seek-resync.patch
git apply 0014-bluray-slideshow-audio.patch
git apply 0024-bluray-popup-atomic-overlay.patch
git apply 0025-bluray-menu-audio-resume.patch
git apply 0026-bluray-popup-fragment-hold.patch
git apply 0027-bluray-menu-audio-diagnostics.patch
git apply 0028-bluray-menu-audio-queue-retain.patch
git apply 0029-bluray-audio-sample-rate.patch
git apply 0032-dovi-rpu-enhancement-metadata.patch
git apply --include=player/command.c --include=player/command.h 0017-secondary-ass-replace.patch
git apply --include=sub/dec_sub.h 0017-secondary-ass-replace.patch
patch -p1 --fuzz=3 < <(sed -n '/^diff --git a\/sub\/sd_ass.c/,/^diff --git a\/player\/command.c/p' \
  0017-secondary-ass-replace.patch | head -n -1)
```

说明：
- 0010 的文档 hunk（options.rst）基于 Yaozhi 私有文档状态，需排除；
- 0015 只取 sd_ass.c 部分（vo_gpu_next.c 部分依赖 luna 蓝光补丁，已由 0001 覆盖）；
- 0017 需拆三段：command.c/command.h 与 dec_sub.h 用 git apply --include，
  sd_ass.c 段与 0015/0016 上下文漂移需 GNU patch --fuzz=3（顺序：0016 之后）；
- 0022/0023（呈现遥测）依赖 Yaozhi 未公开的私有件，未引入。

## 本机构建方法（MSYS2 MinGW64）

依赖（`pacman -S`）：`mingw-w64-x86_64-toolchain meson ninja pkgconf nasm ffmpeg libass
libplacebo shaderc luajit lua51 libarchive libbluray libdvdnav libdvdread uchardet
rubberband zimg lcms2 libjpeg-turbo`

```
source /d/tools/msys64/shell.cfg   # 或 bash -lc
export PATH=/mingw64/bin:$PATH
cd D:/GitHub/mpv-src                # 已应用补丁的源码树（分支 fntv-core-1062）
meson setup build-mingw --buildtype=release -Djavascript=disabled -Dtests=false
ninja -C build-mingw                # 产物 build-mingw/mpv.exe + libmpv-2.dll
# 运行时 DLL：ldd 列出全部依赖，从 /mingw64/bin 拷贝（约 132 个）
```

## 未引入（待办）

- `0004/0008/0012`：libbluray 侧补丁（HDMV IG PID 上限扩展、Link 命令终止、静止菜单 IG 保持）
  —— 需单独重编 libbluray（`src/libbluray/decoders/`、`m2ts_filter`），后补；
- `ffmpeg-9005`（mpegts force_hdmv）与 av3a 系列：需重编 FFmpeg，Audio Vivid 解码待此项；
- `0033`：版本品牌补丁，刻意排除（保持 fntv 身份）；
- `0034/0037-0042`：1.0.6-2 之后的实验系列（continuous-clock、native-ass 节奏演进），
  未进发行版，暂不引入。

## 回滚

- 分支回退：mpv-src `git checkout fntv-danmaku`（仅弹幕系列旧栈）重编即可；
- 更早核心备份在 `third_party/fntv-mpv/_legacy-v157/`（mpv.exe v0.41.0-157）。
