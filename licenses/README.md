# 随包第三方许可副本（Bundled Third-Party Licenses）

本目录存放**第三方组件的许可声明与全文**，用于补齐随安装包分发但来源不在本仓库内的资源
（`third_party/` 按入库规则不入库，其原有的许可文件在重新 clone / CI 构建时会缺失）。

## 为什么需要这个目录

AGPL-3.0 §4/§6 要求分发目标码时随附许可证副本；GPL/LGPL/OFL/Apache 等组件也有同类义务。
`third_party/fntv-mpv/portable_config/` 里的字体（Noto CJK、Material Icons、uosc 图标字体等）
原本没有任何许可文件，或只有 Inter 一份。把副本放在这里可以：

1. 随仓库长期保留，不依赖本地 `third_party/`；
2. 由 `tools/stage-bundle-resources.mjs` 复制进安装包（`bundle-resources/licenses/`）。

## 内容

| 文件 | 对应组件 | 许可 |
| --- | --- | --- |
| `fonts/Baloo2-OFL.txt`（在 `src/assets/fonts/`） | 品牌字体 Baloo 2（内嵌 woff2） | SIL OFL 1.1 |
| `fonts/Inter-LICENSE.txt` | Inter Medium / SemiBold | SIL OFL 1.1 |
| `fonts/NotoSansCJK-OFL.txt` | Noto Sans CJK Bold | SIL OFL 1.1 |
| `fonts/Material-Design-Iconic-OFL.txt` | Material Design Iconic Font | SIL OFL 1.1 |
| `fonts/MaterialIconsRound-LICENSE.txt` | Material Icons Round | Apache-2.0 |
| `fonts/uosc-fonts-LICENSE.txt` | uosc_icons.otf / uosc_textures.ttf | LGPL-2.1 |

其余组件的许可副本随其源码/资源目录一起分发，不必在此重复：

- `LICENSE` — AniBox 本体（AGPL-3.0，660 行全文）
- `THIRD_PARTY_NOTICES.md` — 全部第三方组件的归属与许可清单
- `src-tauri/resources/danmu-api/LICENSE` — danmu_api（AGPL-3.0）
- `third_party/fntv-mpv/portable_config/scripts/THIRD-PARTY-LICENSES.md` — mpv Lua 脚本
- `third_party/fntv-mpv/portable_config/shaders/LICENSE-shaders.txt` — GLSL 着色器
- `third_party/fntv-mpv/portable_config/scripts/uosc/LICENSE.LGPL` — uosc
- `third_party/fntv-mpv/portable_config/scripts/autosubsync/LICENSE` — autosubsync
- `third_party/fntv-mpv/portable_config/script-assets/startup-format-logos/README.md` — 徽章资产
