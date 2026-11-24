# Tigerest-Theater 可移植清单

> 源仓库：`D:\GitHub\播放器\Tigerest-Theater`（v3.0.0，2026-11-19，浅克隆）
> 调研日期：2026-11-23 ／ 落地日期：2026-11-24
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

## 2. 已落地（2026-11-24）

### 2.1 着色器每帧耗时实测 ✅ lc-2341

| | |
|---|---|
| 我们的产物 | `tests/measure_mpv_shaders.py`（串行跑 52 个预设，每个 3 次基线取中位数）、`tests/measure_shader_probe.lua`（探针）、`src/data/shader-timings.json`（数据）、`ShaderGuideModal` 性能列 |
| 做法 | 起 1920×1080 可见窗口播合成 4:2:0 测试图（放大用 1066×600，缩小用 4K），等每个 pass 攒够 120 帧，取最近 48 帧中位数求和，减去同场景基线 |
| 我们额外做的 | 除了「每帧耗时」，还测 **首帧额外延迟**（`compile_ms`）—— 这才是用户感知的代价 |

**实测推翻了清单原来的假设**（这是本轮最重要的发现）：

| 预设 | 每帧耗时 | 首次编译额外等待 |
|---|---|---|
| Anime4K Mode A (HQ) | 1.16 ms | +540 ms |
| FSRCNNX/2x56+SSim+Krig | 0.20 ms | +1.2 s |
| ArtCNN/16 | 0.85 ms | **+14.6 s** |
| ArtCNN/32 | 5.76 ms | **+103 s** |
| CuNNy/4x24 | 2.08 ms | **+15.3 s** |
| CuNNy/4x32 | 3.60 ms | **+35.8 s** |
| CuNNy/8x32 | 6.99 ms | **+72.9 s** |
| AMD FSR / RAVU / CfL / Krig | 0.00~0.10 ms | 测不出差异 |

- 52 个预设全部测到（零 null）。RTX 5070 上，除 CuNNy/ArtCNN 大档外**每帧开销全部落在 0.1~2ms**，也就是 60fps 预算（16.7ms）的零头。清单里「开销：高」这类标签在这张卡上严重失真。
- 真正让用户难受的是**编译**：ArtCNN/16 每帧只多 0.85ms，但点下去要等 14.6 秒；ArtCNN/32 要等 103 秒；CuNNy/8x32 要等 73 秒。原来 UI 里只写「极高」完全没说清这一点。
- 所以 UI 同时显示两个数，且首次编译用暖色单独标出（`ShaderGuideModal` 的 `.sg-compile`）。
- **数据只对本机有效**：JSON 里带 `device` + `measured_at`，弹窗底部如实写明「换显卡或分辨率数字会变，只适合同机横向比较」。

### 2.2 处理顺序流 ✅ lc-2341

`tools/mpv-scripts/fntv_pipeline.lua` 采集 → 写 `user-data/anibox/pipeline` → modernz 视频详情弹窗读一行。

```
640×360 24fps → 软解 → 着色器 2 个 → 缩放 lanczos 2.00x → 去色带 3 → 内置平滑 sphinx → 1280×720
```

只列出真正发生的环节（没开去色带就不写），不给一串「关」。

**踩到并修掉的三处**：
1. `video-format` 在合成源上返回 `WRAPPED_AVFRAME`，编码名要从 `track-list[].codec` 取，且 `wrapped_avframe`/`rawvideo` 要过滤掉；
2. `osd-dimensions` 在裸窗口下报的是「理论占满尺寸」不是真实渲染像素（窗口 1280×720 时它报 1920×1080）→ 改从 `vo-passes` 最后一个 pass 的 w/h 取；
3. **`expand('~~cache/')` 失败时返回空串，而 `string.find(s, '', 1, true)` 恒等于 1** → 所有着色器都被判成 UI 着色器、计数永远是 0。拿不到 cache 目录时直接不过滤。

### 2.3 插帧 ResumePrime ✅ lc-2342

`fntv_interp.lua` 文件尾追加：空闲 >5s 后恢复 → 立刻重新暂停 → seek 回原位 → 等首帧渲染出来 → 放行。

- 只在 rife/mvtools/svp 挂载时做（内置平滑没有推理图）
- 3s 超时无条件放行（不能把用户永久卡在暂停）
- **自锁防护**：自己写的 `pause=true` 不进「用户暂停」计时 —— 否则每次恢复都会再 prime 一次
- seek / end-file 时取消在途 prime（否则令牌失效后 pause 永久停在 true）

验证：`tests/verify_resume_prime_2338.lua`（vm 式抽段 + 假 mp 桩），8/8 通过；**删掉实现后测试直接报错**（双向对照已做）。

### 2.4 去色带参数 A/B ✅ lc-2343

改用 Tigerest 动漫档 `iterations=3 / threshold=48 / range=18 / grain=6`（纯取值，不受许可限制）。

本机实测（RTX 5070，其余条件不变）：

| | 1080p 输出 | 4K 输出 |
|---|---|---|
| 原 `iterations=2` | 0.410 ms | 1.995 ms |
| 新 `3/48/18/6` | 0.418 ms | — |
| 最激进 `iterations=4` | 0.425 ms | 2.194 ms |

从最保守到最激进只差 8μs~0.2ms —— **成本可忽略**，而默认 2 次迭代在天空/暗部留下的色带是能看见的。所以直接采用更激进的档。

---

## 3. 明确不做 / 未实装

| 项 | 原因 |
|---|---|
| **VFR 稳健帧率**（原计划 lc-2339） | **写完自证是 no-op**：只要 `container-fps > 0`，无论 `estimated-vf-fps` 是多少，返回值恒等于 `container`，等于什么都没改。真正修口径要 mpv 逐帧 pts 差分，而 `fntv_aq` 的 bpppf 只用来开关去色带、不是精确预算 —— 成本远超收益。已在 `patch-anibox-mpv.mjs` 里写明，避免后来者重复走 |
| **mpv.conf 编辑器** | 他们 96KB 代码 + 一整套校验；我们还要处理 `mpv-user.conf` 的双写者冲突。工作量超出本轮，单独排期 |
| **专业模式实时面板** | 只搬了其中「处理顺序」这一小块（价值最高、数据全现成）。完整的边看边调面板没有明确需求 |
| **NVIDIA VFG 帧生成** | 要 NVIDIA `nvidia-vfx` wheel（436MB，私有 runtime），包内 +约 200MB，还跑在他们的私有 VapourSynth 运行时上。我们已有 RIFE / MVTools / SVP 三档 |
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

本轮（2026-11-24）已把原清单里价值最高的四项做完：**2.1 耗时实测、2.3 处理顺序流、2.4 ResumePrime、2.5 去色带参数**。

**下一项建议：mpv.conf 编辑器**（原 2.2）—— 这是剩下唯一的高价值项，也是最大的坑（他们 96KB 代码 + 一整套校验）。开工前必须先定两件事：

1. `mpv-user.conf` 的归属：现在它既是我们随包发的（渲染预设/音频响度托管块），又写着「由设置面板自动生成」的注释 —— 实际上没有任何代码在生成它。做编辑器前必须先把这层关系说清，否则两个写者打架。
2. 校验表来源：他们用 `snapshot_mpv_options.py` 从运行中 mpv 抓选项表。我们可以照做（我们已经在 `tests/gen_shader_table.py` 上有同源一致性机制可复用）。

**不要再做的事**：VFR 稳健帧率（`fntv_aq` 的 bpppf 只用于开关去色带，改口径没有收益，已是 no-op 状态）、完整专业模式面板（没有明确需求）。
