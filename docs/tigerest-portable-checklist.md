# Tigerest-Theater 可移植清单

> 源仓库：`D:\GitHub\播放器\Tigerest-Theater`（v3.0.0，2026-11-19，浅克隆）
> 调研日期：2026-11-23
> 结论口径：**每一项都写明「我们已有什么 / 还差什么」**，不把已有的当缺的。

## 0. 许可红线（先看这条）

| | |
|---|---|
| 主项目 | **GPL v2**（README 第 412 行「主项目许可证见 LICENSE（GNU GPL v2）」；`LICENSE` 是 GPL-2.0 原文） |
| 我们 | AGPL-3.0 |

**GPLv2 与 AGPL-3.0 不兼容** —— 他们的 C++/Lua/mpv.conf **文本不能直接复制**进本仓库。

（严格说：GPL-2.0 若带 "or later" 可由 GPLv3 桥接到 AGPLv3，但本项目源码文件头没有任何 GPL 声明、README 也只写 "GNU GPL v2"，按最保守口径处理 —— 不复制文本。

可以做的是：
- ✅ **读思路后重写**（本文档的做法）
- ✅ 借鉴公开的**算法/公式/参数值**（不受版权保护）
- ✅ 使用**同一上游**的第三方资源（着色器原件来自 Anime4K / ArtCNN / FSRCNNX / RAISR / SSim / CfL，需各自核对许可）
- ❌ 复制他们的 `.lua` / `.cpp` / `.conf` / `.js` 文本
- ❌ 复制他们的中文注释文案

---

## 1. 已完成

### 1.1 RTX 视频增强（超分辨率 VSR / RTX Video HDR）✅ 2026-11-23 已落地

| 项 | 状态 |
|---|---|
| 实现方式 | mpv 的 `d3d11vpp` 滤镜：`scaling-mode=nvidia` / `scale=N` / `nvidia-true-hdr=yes` |
| 依赖 | **零**（随包 mpv v0.41.0-1049 已编译 `d3d11vpp`，二进制 grep 命中 `nvidia-true-hdr`/`scaling-mode`） |
| 本机验证 | RTX 5070 日志确认 `NVIDIA RTX Super Resolution enabled.` 与 `NVIDIA RTX Video HDR enabled.` 均由驱动真实启用 |
| 我们的产物 | `src-tauri/src/rtx.rs`（配置/系统 HDR 检测/命令）、`tools/mpv-scripts/fntv_rtx.lua`（挂载决策 + 链尾维护）、`src/styles/51-play-controls.css` 无关；设置面板 `RtxVideoRow` |
| 参考位置 | `src/player/PlayerComponent.cpp:5148-5330` |

**从他们身上学到的三条硬规则**（我们已全部实现）：
1. **滤镜必须永远排在链尾** —— 插帧（RIFE/MVTools/SVP）、AI 超分、弹幕 `lavfi` 都会改 `vf` 链；RTX 若被甩到前面，插帧输出带着 HDR 全范围标签进 `d3d11vpp`，驱动会**自禁**
2. 放大倍数按**屏幕物理像素 / 画面显示尺寸**算，上限 4×
3. HDR 只在「SDR 源 + 系统 HDR 已开」时挂，其它情况必须**给出原因**（片源本身 HDR / 系统 HDR 没开）

**我们额外修的两处（他们没踩或没写）**：
- 系统 HDR 检测：他们从 mpv 属性推断，我们实测发现 **`video-target-params/gamma` 在 SDR 片源上恒为 `srgb`**（系统 HDR 开着也不变 `pq`）→ 改为起播时由 Rust 走 `DisplayConfig`（`GET_ADVANCED_COLOR_INFO` 的 advancedColorEnabled 位）问一次 Windows
- 参数变化必须重挂：只判「在不在链尾」会漏掉「系统 HDR 刚打开」这类只改参数的情况

---

## 2. 值得做（按价值排序）

### 2.1 着色器每帧耗时实测 —— 高价值，成本低 ⭐ 推荐下一项

| | |
|---|---|
| 他们的文件 | `tools/measure_mpv_shaders.py` + `resources/mpvconf/shader-timings.json` |
| 我们现状 | `src/lib/shaderGuide.ts` 的性能列是**手写档位**（低/中/高/极高），无任何一个数字 |
| 差距 | 用户选着色器时看到的是「开销：高」这种主观标签，不知道自己机器上到底多少 ms |

**做法**（可完整重写，只借思路）：
- 起一个可见 mpv 窗口，播合成 4:2:0 测试图（放大类用 1066×600 源 1.8× 放大到 1920×1080，缩小类用 3840×2160）
- 等每个渲染 pass 累计 ≥120 帧的计时样本，取**最近 48 帧的中位数**求和（避开首帧编译抖动）
- 减去「不加着色器」的基线 → 得到该着色器的净 GPU 耗时
- 写 JSON，UI 显示真实 ms

**关键细节**（他们踩过/写明的）：
- 用 `vo-passes` 的 `fresh` 与 `redraw` 两组，分别汇总
- `//!WHEN` 条件在当前缩放下不成立的 pass 要跳过（否则给 0）
- 他们测的是 M4 Pro / macOS，**我们不能直接抄数字**，必须在本机自测
- 1 ms 以下的差异受显卡降频影响，不能用来排序

**我们已有的可复用基础**：`third_party/.../scripts/stats.lua` 是 mpv-stats 汉化版，里面已有 `vo-passes` 读取逻辑可参考写法（但不要依赖它，它是交互式面板）。

---

### 2.2 mpv.conf 编辑器（用户可直接改配置 + 一键交给 AI）—— 高价值，成本高

| | |
|---|---|
| 他们的文件 | `native/mpvConfEditor.js`（55KB）、`src/player/MpvConfDocument.cpp`（41KB，纯文本逻辑）、`MpvConfEditor.cpp` |
| 我们现状 | **完全没有**。`mpv.conf` / `mpv-user.conf` 只能改仓库里那份（打包时随 staging 进去），用户改不了 |
| 他们做到的程度 | 管理区（只读头尾）+ 用户区 + 锁定键表 + 条件暂停行 + 逐行校验 + 一键「复制给 AI」（生成修改规则 + 完整 conf）+ 恢复默认 + 粘贴时 diff 确认 |

**他们的设计骨架（可照搬思路）**：
- 文件分三段：`head`（软件注入、只读）/ `user`（用户可编辑）/ `tail`（软件注入、只读）
- 每个配置键有**归属表**：`Free`（可改）/ `Locked`（软件管，如 `vo`/`gpu-api`/`include`）/ `Page`（该去设置页改）
- 冲突行（如开了 RIFE 时改帧率的滤镜）标黄 + **暂停执行**，不报错
- 校验靠**运行中 mpv 的选项表**（`snapshot_mpv_options.py` 抓成 JSON），不是硬编码

**我们的额外优势**：`StatsPanel` 里已有 `shader_preset_table()`（52 个预设）与 `fntv_shader.lua` 的 `PRESETS` 双表，`tests/gen_shader_table.py` 已在保证两边一致 —— 做编辑器时可以直接复用这套一致性机制。

**风险**：我们有 `mpv-user.conf` 由设置面板自动生成（ICC/渲染预设），编辑器必须把这份也划进「只读管理区」，否则两个写者打架。

---

### 2.3 专业模式实时面板（边看边调 + 处理顺序可视化）—— 中高价值，成本中

| | |
|---|---|
| 他们的文件 | `native/proMode.js`（126KB）、`resources/mpv/plugins/tigerest_pro.lua`（7KB） |
| 我们现状 | **有分离的两块**：`fntv_mediainfo.lua`（左上角徽章：硬解/动态范围/编码/分辨率/帧率/码率）+ `stats.lua`（Ctrl+i 完整统计页）+ 玻璃底栏的着色器/插帧弹窗 |
| 差距 | 没有「一边调一边看」的联动面板；没有「处理顺序」可视化（输入→硬解→滤镜→着色器→缩放→去色带→插帧→输出） |

**最值得先搬的一小块：处理顺序流**（他们 `proMode.js` 的 `renderFlow()`）

```
输入 1920×1080 → 硬解 d3d11va → RIFE 补帧 → 着色器 ×3 → 缩放 ewa_lanczos → 去色带 → 插帧 → 输出 3840×2160
```

数据来源全是现成 mpv 属性：`video-params`、`hwdec-current`、`vf`（带 `enabled`）、`glsl-shaders` 计数、`deband`/`interpolation`、`osd-dimensions`。
**可以直接塞进我们现有的玻璃 UI**（弹窗或详情页），不需要新面板。

**他们的 stats 采集模式（值得学）**：
- `tigerest_pro.lua` 每 0.5s 采一次，写到 `user-data/tigerest/pro/stats`
- 面板侧 `observe` 该属性 → 拿 JSON → 渲染
- **属性全在脚本线程读，UI 线程不等待 mpv**
- 一次切档会连续改十几个属性 → 用 30ms debounce 合并成一次发布

这个「Lua 采集 → user-data → 前端 observe」的管线我们已经在用（`user-data/anibox/quality-request`、`user-data/fntv_shader/desc`），直接扩展即可。

---

### 2.4 插帧策略的纯逻辑（模型加载门控 / 空闲恢复回填）—— 中价值，成本低

| | |
|---|---|
| 他们的文件 | `src/player/interpolation/InterpolationPolicy.h`（`Eligibility` / `StartupGate` / `ResumePrime`）、`FrameTiming.cpp` |
| 我们现状 | `fntv_interp.lua` + `enhance.rs` 已有：倍速/HDR/超 4K 让路、`video-sync=audio`、吞吐看门狗回退 MVTools、真机验证 `estimated-vf-fps` 翻倍 |
| 他们多做的 | ① **StartupGate**：模型加载期冻结音视频时钟（用户暂停意图与准备暂停分开记）；② **ResumePrime**：空闲 >5s 后恢复，先暂停 → seek 回当前位置 → 等首帧就绪再放（TRT 图闲置会掉 10-25 帧）；③ **FrameTiming**：播放卡顿造成的长帧**不算可变帧率** |

**可行性**：这三条是纯算法，30~50 行 Rust/Lua 各一份。①③ 我们已有类似效果（看门狗），② 我们**没有**（RIFE 长时间暂停后恢复会掉帧）。

---

### 2.5 三档 profile 参数 A/B 参考 —— 低价值，成本极低

他们 `mpv.default.conf` 的三档与我们 `fntv_shader.lua` 的预设可以对照：

| 档 | 他们 | 我们 |
|---|---|---|
| 默认（动漫） | `SSimDownscaler + KrigBilateral + ravu_zoom_ar_r3` | `Anime4K Mode A HQ`（`aq_hq_a`：`Clamp_Highlights + Restore_CNN_VL + Upscale_CNN_x2_VL + AutoDownscalePre_x2/x4 + Upscale_CNN_x2_M`） |
| 真人 | `SSimDownscaler + ravu_zoom_ar_r3`（同链去 Krig） | `fsrcnnx_2/8/16/56` 四档（FSRCNNX + 可选 SSim + KrigBilateral，自带色度放大） |
| 激进测试 | Anime4K 完整链（`SSim + Restore_CNN_UL + Upscale_CNN_x2_UL + Thin_HQ`） | `aq_hq_aa`（Mode A+A 二次重建） |

**结论**：我们预设更多（51 个 vs 3 个），不需要照搬。但他们的**去色带参数**值得 A/B：

| | 公共基础 | 默认（动漫）档 |
|---|---|---|
| 他们 | `iterations=4 / threshold=48 / range=18 / grain=6` | `3 / 24 / 12 / 12` |
| 我们 | `deband-iterations=2`（其余走 mpv 默认） | — |

我们的值明显更保守。**可试动漫档用他们的 `3/24/12/12` 看天空/暗部色带是否改善** —— 这是纯取值，不涉及许可。

---

## 3. 明确不做

| 项 | 原因 |
|---|---|
| **NVIDIA VFG 帧生成** | 要 NVIDIA `nvidia-vfx` wheel（436MB，私有 runtime），包内 +约 200MB，还跑在他们的私有 VapourSynth 运行时上。分发成本与工程量都不划算；我们已有 RIFE / MVTools / SVP 三档 |
| QtWebEngine fence patch | 他们治的是 QtWebEngine 共享纹理花屏，我们不用 QtWebEngine |
| Emby / Jellyfin 后端对接 | 我们是飞牛影视客户端 |
| 一起看 / 公共评论 / 弹幕共享映射 | 都是他们自建服务端，与我们的产品方向无关 |
| libmpv Render API 单窗口合成 | 我们是独立 mpv 窗口 + 双皮肤，架构不同 |

---

## 4. 附带发现：他们的工程做法值得学（与功能无关）

| 做法 | 说明 |
|---|---|
| **发布说明里放全平台下载表** | 每次 release 的 CHANGELOG 与 GitHub release body 都从 `tools/release_download_table.py` 生成，本版不发的平台指向它保持的旧版本 —— 比我们手写「仅 Windows」清楚 |
| **CLI 工具链齐全** | `snapshot_mpv_options.py` / `measure_mpv_shaders.py` / `generate_mpv_shader_catalog.py` / `release_download_table.py` —— 都是「把人工步骤变成可重跑脚本」，我们的 `tests/gen_shader_table.py` 是同一思路 |
| **运行时资源按 manifest 校验** | `resources/mpv/shaders/manifest.json` 记录每个着色器 sha256，升级只替换/删除随包文件、绝不碰 `shaders/user/` —— 我们目前没有这层保护 |
| **文档带实测数据** | `docs/verification/*.md` 里每篇都是一个具体问题 + 根因 + 修法 + 前后对照表（如 D3D11 fence sync 那篇给了 6 组抓帧统计） |

---

## 5. 行动建议

**下一项做 2.1（着色器耗时实测）** —— 成本最低、收益最直接：现有 UI 里 52 个预设的「开销」标签会从主观变成实测 ms，用户选档位时第一次有真实依据。

我们随包 **57 个 `.glsl`**（他们 87 个，差在 Anime4K 全集与 ArtCNN 变体）。每个约 20s（等 120 帧样本 + 编译），**一次连续跑约 20 分钟**。跑法要遵守本机纪律：串行、不并发、跑完 `tasklist` 确认无残留 mpv。

**2.3 的「处理顺序流」** 可以顺手做，数据全现成、不需要新面板。

**2.2 的 mpv.conf 编辑器** 是最大的坑（他们用了 96KB 代码 + 一整套校验），要单独排期。
