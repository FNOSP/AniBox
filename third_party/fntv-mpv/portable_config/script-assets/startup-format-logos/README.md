# 起播格式徽章资产（startup-format-logos）

来源：[mpv-Yaozhi](https://github.com/Yaozhil/mpv-Yaozhi) 1.0.6-2 整合包
（`script-assets/startup-format-logos/runtime/`，697 个文件）。

许可：mpv-Yaozhi 作者独立创作的自绘徽章资产按其仓库 [MIT License](https://github.com/Yaozhil/mpv-Yaozhi/blob/main/LICENSE.md)
分发（Copyright (c) 2026 Yaozhil），本目录随 Fntv-Plus 原样捆绑并保留本声明。

## 结构

- `manifest.json`：布局与索引（29 个格式 slug × 彩色/透明白 × 1x/2x 两档 × 6 级透明度渐变帧）
- `logo-<slug>-<透明度>.bgra`：2x 档（312×128，1440p/4K 用）
- `logo-<slug>-<透明度>-1x.bgra`：1x 档（156×64，1080p 及以下 1:1 渲染）
- `-white-`：透明白图标变体（对应设置里的 style=white）

渲染走 mpv 标准 `overlay-add` 命令（原始 BGRA + dw/dh 缩放），由
`scripts/fntv_format_badge.lua` 读取本 manifest 驱动；资产缺失时自动回退
ASS 文字胶囊渲染。
