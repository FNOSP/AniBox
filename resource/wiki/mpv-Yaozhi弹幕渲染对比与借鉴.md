# mpv-Yaozhi 弹幕渲染对比与借鉴方案

> 2026-10-10 · 针对「mpv 弹幕滚动一顿一顿」问题的竞品分析与可借鉴点清单
> 素材来源：`D:\GitHub\mpv-Yaozhi`（公开仓库，仅配置说明）+ 官方整合包 `mpv-Yaozhi-1.0.6-2.7z`（完整脚本，已解包至 `D:\GitHub\mpv-Yaozhi-extracted\`）

---

## 一、结论速览

| 维度 | 我们 (Fntv-Plus) | mpv-Yaozhi | 差距判定 |
| --- | --- | --- | --- |
| mpv 核心 | `v0.41.0-157` | `v0.41.0-1050` 自编译（含弹幕专用补丁×3，**补丁源码未公开**） | 核心不可直接抄，但可自己补 |
| 渲染默认路径 | OSD overlay + Lua 定时器逐 tick 重算 `\pos` | **副字幕 ASS 轨道**（libass 按帧插值，零 Lua 定时器）+ overlay 兼容回退 | 他们默认路径根治抖动；依赖其私有核心补丁 |
| OSD 回退路径 | 原版 uosc_danmaku 水平 | 大改 V19.5：墙钟外推 + 绝对 deadline 网格 + 预计算运动参数 + 二分窗口 + 双 overlay 缓存 + 自适应刷新 | **纯 Lua，可直接移植，对我们收益最大** |
| time-pos 量化问题 | 存在（24fps 片源弹幕 24Hz 步进） | 已解决（墙钟外推） | ★ 卡顿根因 #1 |
| 定时器相位漂移 | 存在（add_periodic_timer 从实际回调时间重排） | 已解决（一次性 timer + 绝对 deadline 网格） | ★ 卡顿根因 #2 |
| 每 tick 正则解析 `\move` | 存在（每条活动弹幕每 tick 一次正则） | 已解决（parse 阶段预计算 move/pos 表） | ★ 卡顿根因 #3 |
| 每 tick 全量扫描 COMMENTS | 存在（上万条全遍历） | 已解决（binary_search 定位窗口起点） | ★ CPU 大头 |
| 顶/底固定弹幕重画 | 每 tick 与滚动弹幕一起重画 | 独立 overlay，仅成员变化时更新 | CPU 优化 |

---

## 二、他们的架构（V19.5.x）

### 2.1 默认路径：Native ASS 副字幕轨（`render_mode=auto` → `ass-track`）

弹幕写成 ASS 文件后用 `sub-add` 挂成**真正的副字幕轨**（`secondary-sid`），
动画完全交给 libass 按帧 pts 插值 `\move`——脚本侧**没有任何渲染定时器**。
`render.lua` 里的原话：

> A real ASS subtitle track lets mpv/libass own the animation timeline,
> **avoiding the regular micro-jitter caused by a Windows/Lua software timer racing the display VBlank.**

（Windows/Lua 软件定时器与显示 VBlank 竞争导致的规律性微抖动——正是我们的「一顿一顿」。）

但这依赖他们自编译核心的三个私有补丁（`mpv.exe` 内实测存在，上游 mpv 没有）：

| 补丁属性/命令 | 作用 | 缺了会怎样 |
| --- | --- | --- |
| `secondary-sub-ass-refresh-rate` | 副字幕 ASS 按**显示器刷新率**重绘（如 120Hz），不受视频帧率限制 | 24fps 视频上弹幕只有 24Hz 步进（比我们的 overlay 还卡） |
| `secondary-sub-ass-full-viewport` | 副字幕渲染铺满**整个输出画面**而非视频矩形 | 弹幕被限制在视频区内，黑边区没有弹幕 |
| `secondary-ass-replace <id> <path>` | 弹幕更新时**原位原子换轨** | live 更新要 sub-remove/sub-add，肉眼可见重载闪烁 |

我们的核心 `v0.41.0-157` 三个都没有（已用二进制实测确认）。
**注意：他们公开仓库不含这三个补丁的 C 源码**，README 明说完整包才带、且只发二进制。

### 2.2 兼容路径：改进版 OSD overlay（用户选了第二字幕、全屏黑边、ass-track 失败时）

这部分**不依赖任何私有补丁**，是我们最值得抄的部分（详见第三节）。

### 2.3 路径选择逻辑（`desired_render_mode`）

```
render_mode=auto
  ├─ 用户选了真实第二字幕        → overlay（兼容，非失败，条件恢复后可回 ass-track）
  ├─ 全屏黑边且核心支持 full-viewport → ass-track（弹幕放黑边、铺满全屏）
  ├─ 全屏黑边（旧核心）          → overlay
  ├─ HDR/PQ/HLG                 → ass-track（不碰视频管线，HDR 安全）
  └─ 其余                       → ass-track
```

他们的 fps 补帧滤镜（`@danmaku:fps`）已降级为「显式选 ass-track 且 SDR」才可能启用的
遗留选项（默认 `vf_fps=no`），并硬性保证 PQ/HLG 下永不插入。

---

## 三、可直接移植的 Lua 级改进（不依赖私有核心，按收益排序）

### ★1. 墙钟外推 —— 治 `time-pos` 量化（最大嫌疑根因）

**问题**：我们每 tick（60Hz）读 `mp.get_property_number('time-pos')`。
`time-pos` 只在**新视频帧呈现时**更新——23.976fps 动画片上它每秒只变 24 次。
于是 60Hz 定时器连续 2~3 tick 拿到同一个值 → 弹幕位置连停两三帧再跳一下 →「一顿一顿」。
（他们代码注释原话：*"time-pos is quantized to decoded video frames. Extrapolate from a
monotonic playback anchor so a 60 Hz OSD overlay does not repeat the same coordinate
two or three times on 24/30 fps sources."* 我们看动画，全是 23.976fps，命中最狠的一档。）

**他们的做法**（`render.lua` 的 overlay clock）：

```lua
-- 播放时维护单调锚点，位置 = 锚点 + (当前墙钟 - 锚点墙钟) × 倍速
local position = overlay_clock_media
    + ((sample_wall or mp.get_time()) - overlay_clock_wall) * overlay_clock_speed
-- speed 取 speed × video-speed-correction
-- 以下时机显式 rebase 锚点：seeking、playback-restart、pause/恢复、
-- speed 变化、paused-for-cache(缓存停顿)、倍速校正变化
```

暂停/缓存停顿时冻结提交给 libass 的位置（不是冻结锚点），恢复时不回跳。

### ★2. 绝对 deadline 网格定时器 —— 治相位漂移

**问题**：`mp.add_periodic_timer` 从**实际回调时刻**排下一轮；一次迟到会把后续
所有 tick 永久后移，位置步长忽长忽短 → 水平运动不均匀。这正是我们 lc-1272/lc-1303
反复调步进值仍「时不时一顿」的另一个来源。

**他们的做法**：一次性 `add_timeout` + 绝对 deadline：

```lua
-- 每轮：跳过已过期槽位（不补画），把 deadline 推回网格
local skipped = math.floor(math.max(0, started - deadline) / interval)
deadline = deadline + skipped * interval
-- ...渲染（采样时刻用 deadline 而不是实际回调时刻！回调抖动不进位置）...
repeat deadline = deadline + interval until deadline > now
timer.timeout = math.max(0.00025, deadline - now)   -- 不丢即将到来的槽位
timer:resume()
```

要点：**位置采样时刻用 deadline**（理论拍点），而不是回调发生时刻——
回调迟到不会复制进弹幕的空间步长。

### ★3. parse 阶段预计算 —— 消灭每 tick 正则

**问题**：我们 `render()` → `parse_comment()` 对每条活动弹幕每 tick 跑一次
`text:match("\\move%(...)")` 正则；几百条弹幕 × 60Hz = 每秒几万次正则，纯烧 CPU。

**他们的做法**：`parse.lua` 生成事件时就带结构化字段，
`render.lua` 只在首次遇到某事件时算一次并缓存在事件对象上：

```lua
-- parse 阶段
event.move = {x1, y, x2, y}   -- 或 event.pos = {x, y}
event.layer  = (style == "R2L") and 0 or 1

-- 渲染首见时
event._overlay_motion_x, event._overlay_motion_y = unpack(event.move)
event._overlay_motion_dx = x2 - x1
event._overlay_motion_inv_duration = 1 / math.max(0.001, event.end_time - event.start_time)
event._overlay_text = 去掉\move后的文本        -- 缓存
-- 之后每 tick：current_x = x + dx * (pos - start) * inv_duration   纯算术
```

### ★4. binary_search 窗口 —— 消灭全量扫描

**问题**：我们 `render()` 每 tick `ipairs(COMMENTS)` 遍历**全部**弹幕（实测有 1.6 万条的场），
60Hz 下每秒上百万次比较。

**他们的做法**：按 start_time 排序后，每 tick 二分定位「最早可能还活着的弹幕」：

```lua
local window_start = pos - math.max(options.scrolltime, options.fixtime)
local lo = binary_search(COMMENTS, window_start, function(item) return item.start_time end)
for i = lo, #COMMENTS do ... 遇到 start_time > pos 就 break end
```

### ★5. 双 overlay（滚动/固定分离）+ 变更缓存

滚动弹幕（layer 0）每 tick 变 → 一个 overlay；顶/底固定弹幕（layer 1）内容不变 →
另一个 overlay（更高 z），**仅成员集合或样式变化时才 update()**。
且 payload 与上次相同就跳过 `overlay:update()`（他们连 `res_x/res_y` 变化都纳入缓存键）。
我们每 tick 把固定弹幕和滚动弹幕拼进同一串整段重建。

### ★6. 空闲休眠

无活动弹幕的间隙，定时器睡到下一个事件边界（有界 1s 兜底），
而不是像我们 60Hz 空转重建空字符串。弹幕稀疏段 CPU 直降。

### ★7. 增量 GC

每 tick 大量短命字符串会造成周期性 GC 停顿（表现为弹幕隔几秒顿一下）。
他们每轮渲染后 `collectgarbage('step', 32)` 摊平回收成本。

### 8. 自适应刷新健康监控（可选，锦上添花）

- 每 2s 采样 `frame-drop-count` + `vo-passes` 渲染预算占比；
- **只有弹幕自身吃掉 ≥78% 预算**才判定压力（丢帧若是显示合成/解码/降频造成，降弹幕刷新既救不回帧还白牺牲流畅度）；
- 降速只降「一档显示整数分频」（120→60、144→72），稳定 20s 后恢复；
- 单窗口丢帧尖峰（开统计面板等一次性 OSD 重排）不计入，防误降；
- 状态发布到 `user-data/uosc_danmaku/overlay-cadence` 供 uosc/诊断读取。

### 9. 其他小件

- **overlay 保温**（`park_overlay_renderers`）：模式切换前先以透明 payload 预热
  libass 上下文，全屏切换时避免字体构造的首次卡顿；
- **手动 overlay_fps 也按显示刷新整数分频对齐**（如手动 50Hz @ 144Hz 屏 → 48Hz），
  避免异步时钟拍点漂移；
- >180Hz 面板自动封顶 ~144Hz，防 Lua/ASS 重绘工作量无上限；
- `max_screen_danmaku=120` 同屏上限（我们已有 limit_danmaku，等价物存在）。

---

## 四、我们当前实现的具体病灶对照（render.lua 逐条）

| 我们的位置 | 问题 | 对应移植项 |
| --- | --- | --- |
| `render.lua:392` `mp.get_property_number('time-pos')` 每 tick 轮询 | 24fps 量化 → 24Hz 步进 | ★1 墙钟外推 |
| `render.lua:438` `mp.add_periodic_timer(INTERVAL, render, true)` | 相位漂移 | ★2 deadline 网格 |
| `render.lua:413` `for _, event in ipairs(COMMENTS)` 全量遍历 | O(全部弹幕)/tick | ★4 二分窗口 |
| `render.lua:39 parse_move_tag` 每 tick 对每条活动弹幕跑 `\move` 正则 | O(活动弹幕)次正则/tick | ★3 预计算 |
| `render.lua:425` 每 tick 重建整段 ASS 字符串（含固定弹幕） | 字符串 churn + GC | ★5 双 overlay + ★7 增量 GC |
| `render.lua:582` display-fps 观察器重建 timer，但仍是 periodic | 步进对了，相位没对 | ★2 |
| `render.lua:411-430` 单 overlay 混装滚动+固定 | 固定弹幕陪着重画 | ★5 |

（行号为 2026-10-10 版本，行号会漂移，以函数名定位。）

---

## 五、建议的实施路线（2026-10-11 更新：第一、二步已完成）

### 第一步（纯 Lua 移植）→ ✅ 已由更优方案取代并完成
Yaozhi 的 `codex/danmaku-*` 分支里发现了**全部补丁的源码**（`build/bluray-menu/patches/`，
git format-patch 格式），无需逆向。核心侧（下述第三步内容）已提前完成；
Lua 侧已把「原生 ASS 副字幕轨渲染」（他们的 ass-track 模式）适配进我们的
`modules/render.lua`（lc-1351）：
- 弹幕 sub-add 为副字幕轨 + `secondary-sid` 接管，libass 按显示刷新率渲染，脚本零定时器；
- 更新走 `secondary-ass-replace` 原位换轨；全屏黑边自动开 `full-viewport`；
- 用户选第二字幕 → 临时回退 OSD 并复原其副字幕状态；旧核心自动回落旧 OSD 路径（零回归）；
- 沙箱桩件双向验证 29/29（旧核心绝不 sub-add、新核心属性/换轨/回退/清理全链路），
  真机端到端通过（`render_stub_test.lua` 保留为零 CPU 回归测试）。

### 第二步（验证）→ ✅ 完成
用他们的补丁源码直接编译了我们自己的核心 `mpv v0.41.0-1011-gabd7932ba`
（基线 c318236 + 补丁 0010/0015/0016/0017/0018-0031，见 `third_party/mpv-patches/`），
三属性可读写、端到端弹幕渲染出帧验证通过。不再需要逆向。

### 第三步（核心补丁）→ ✅ 完成
见 `third_party/mpv-patches/README.md`：基线 commit、应用顺序、MSYS2 构建方法、
回滚说明（旧核心备份于 `third_party/fntv-mpv/_legacy-v157/`）。
未引入：0034 continuous-clock / 0038 quiet-gap-hold（10 月新实验分支，发行版 1.0.6-2 亦不含）；
Yaozhi 的 overlay 微优化（墙钟外推/deadline 定时器等）暂未移植——轨道模式成为默认后，
OSD 仅作回退路径，收益有限，后续有需要再移植。

---

## 六、附录

- 克隆：`D:\GitHub\mpv-Yaozhi`（公开仓库，仅 mpv.conf/uosc_danmaku.conf 骨架，脚本不提交）
- 完整整合包：`D:\GitHub\mpv-Yaozhi-package.7z`（335MB，1.0.6-2，可删）
- 解包目录：`D:\GitHub\mpv-Yaozhi-extracted\mpv-Yaozhi-1.0.6-2\`
  - 弹幕脚本：`portable_config\scripts\uosc_danmaku\`（render.lua 1826 行 vs 我们 656 行）
  - 实验核心：`portable_config\experimental\omniphony\mpv-v1.0.6-native-refresh-2b60d72__orender-v0.6.0__bridge-v0.8.0\mpv.exe`
- 核心版本实测：他们 `mpv v0.41.0-1050-g38e9a25c1`（含三补丁）；我们 `mpv v0.41.0-157-gdb22a4528`（三属性均无）
