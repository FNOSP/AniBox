# Fntv-Plus 捆绑的第三方 mpv 脚本（来源与许可）

本目录下由 [mpv-Yaozhi](https://github.com/Yaozhil/mpv-Yaozhi) 1.0.6-2 整合包原样捆绑的
社区脚本（未做修改，各自保留原始头部声明与许可）。Yaozhi 仓库对其独立创作内容采用
MIT License；下列第三方脚本以其自身声明的许可为准。

## 脚本清单

| 脚本 | 作用 | 作者/来源 | 许可 |
| --- | --- | --- | --- |
| persist_properties.lua | 音量/倍速/字幕延迟等运行时属性跨会话记忆 | dyphire/mpv-scripts | MIT |
| seek-feedback.lua | 跳转视觉反馈动效 | dyphire/mpv-scripts | MIT |
| autoload.lua | 自动加载同目录媒体（本地连播） | mpv-player 社区 (mpv/autoload.lua) | GPL-2.0+?（随上游） |
| blacklist-extensions.lua | 自动连播扩展名黑名单 | dyphire/mpv-scripts | MIT |
| auto-save-state.lua | 播放中定时自动保存播放状态 | dyphire/mpv-scripts | MIT |
| trackselect.lua | 音轨/字幕智能选择（非配音优先） | po5/trackselect (dyphire 改) | MIT |
| undoredo.lua | 跳转撤销/重做 | Eisa AlAwadhi (UndoRedo) | BSD-2-Clause |
| alist_speed_indicator.lua | 挂载盘（AList 等）下载速度指示 | Yaozhi | MIT |
| sub-fonts-dir-auto.lua | 外挂字体目录自动加载 | Yaozhi | MIT |
| fix-avsync.lua | 切换音频设备后的 A/V 同步修复 | dyphire/mpv-scripts | MIT |
| uosc-subtitle-lines.lua | 字幕行列表跳转（uosc 菜单） | Yaozhi | MIT |
| image-subs-brightness.lua | PGS/图形字幕色彩模式与亮度（150–400 nits） | Yaozhi | MIT |
| chapterskip.lua | 按章节名跳过 OP/ED/广告 + 跳到静音点 | po5/detuur/Eisa01/dyph 等 | MIT |
| evafast.lua | 按住方向键平滑加速快进 | Eisa AlAwadhi (evafast) | MIT |
| dynamic-crop.lua | 实时检测并裁剪黑边 | dyphire/mpv-scripts (b4ndit 改) | MIT |
| pip.lua | 画中画小窗 | Yaozhi | MIT |
| mpv-cropscreen.lua | 截屏/框选截屏/双图拼接 | Yaozhi | MIT |
| sub_export.lua | 导出当前字幕 | dyphire/mpv-scripts | MIT |
| keybinding-manager.lua | uosc 内快捷键搜索/修改/恢复 | Yaozhi | MIT |
| recentmenu.lua | 最近播放列表 | Eisa AlAwadhi (RecentMenu) | MIT |
| autosubsync/ | 字幕自动对轴（纯 Lua） | Yaozhi | MIT |

`scripts/` 目录中 Fntv-Plus 自有脚本（fntv_*.lua、hdr_mode.lua、music-mode.lua、
ontop_mode.lua、smart_skip/、thumbfast.lua、uosc/、uosc_danmaku/）许可见各自仓库与文件头。
