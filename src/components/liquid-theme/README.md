# Liquid Theme Reveal —— 液体主题切换（可复用模块）

深度融合主题切换：新主题像液体一样从按钮下方流出、落底飞溅、积聚成波浪液面、上涨覆盖视口。
液体经过之处（背景 / 卡片 / 文字 / 边框 / 图表）立刻显示新主题。

```
src/components/liquid-theme/
├── types.ts                 公共类型（受控主题、方向、阶段、适配器接口）
├── timeline.ts              阶段边界 + 缓动 + 纯数学（可单测）
├── geometry.ts              每帧液体轮廓 → 一条 SVG path（纯函数，不碰 DOM）
├── palette.ts               需求给定调色板 + 揭示层不透明底 + 湿边色
├── reveal.ts                揭示层引擎：快照克隆 + clipPath + 单 rAF + 清理
├── audio.ts                 Web Audio 本地合成（四段提示 + 跟随 flow 的流水）
├── prefs.ts                 音效开关/音量持久化
├── liquid-theme.css         舞台/揭示层/湿边/指针拦截样式
├── useLiquidThemeReveal.ts  React 接入（受控主题 + 回调）
└── index.ts                 出口（并 import 上面的 CSS）
```

## 在 AniBox 里的接入点

| 位置 | 改了什么 |
|---|---|
| `src/lib/theme.ts` | **新增**。主题作用域的唯一读写实现：`readStoredTheme` / `applyThemeToDocument` / `currentDocTheme` |
| `src/main.tsx` | 挂载前同步写 `<html data-theme>`，避免刷新闪错主题 |
| `src/App.tsx` | 删掉旧 `.theme-veil` 圆形遮罩，改 `useLiquidThemeReveal`；主题写入改 `data-theme` |
| `src/lib/adaptiveTone.ts` | `isDarkTheme()` 从 `.dark` class 改读 `data-theme` |
| `src/styles/01-tokens.css` 等 12 个文件 | `html.dark` → `[data-theme="dark"]`；`:root` 扩成 `:root, [data-theme="light"]`；主题后代规则加"最近作用域"守卫 |
| `src/components/settings/SettingsPanel.tsx` | 外观页新增「主题切换音效」开关 + 音量 + 四段试听 |
| `src/types.ts` / `src/lib/settings.ts` | `themeSound` / `themeSoundVolume`（存 localStorage，不进 Rust config） |

### 为什么主题作用域必须从 class 改成属性

旧实现是 `html.dark` —— **全局单例**，一屏里只能有一套主题。所以旧动画只能盖一层"新主题纯底色"，
圆内的卡片/文字并不变色。

液体揭示要求液体边界两侧**同时**存在两套完整界面，所以主题必须能挂在任意子树：

```
html[data-theme="dark"]           ← 底层：真实界面，旧主题
└ .lt-stage
   ├ .lt-reveal  (clip-path: url(#…))
   │  ├ .lt-backing           不透明底
   │  └ .lt-host[data-theme="light"]  ← 揭示层：新主题作用域的视觉快照
   └ .lt-sheen                湿边（水面高光）
```

**关键细节**：揭示层嵌在 `html[data-theme="dark"]` 里，`[data-theme="dark"] .app-shell` 这类
后代规则会穿过祖先命中克隆体，把浅色层染成深色。所以每条主题后代规则都加了守卫：

```css
[data-theme="dark"] .app-shell:not([data-theme="dark"] [data-theme="light"] *) { … }
```

含义是"不存在『深色里又套了浅色』的祖先"，二值主题下等价于"最近主题作用域胜出"。
验证见 `tests/verify_liquid_theme_scope_2154.cjs`（4 场景 + 反向对照）。

## 最小用法

```tsx
import { useLiquidThemeReveal } from "./components/liquid-theme";
import { applyThemeToDocument, readStoredTheme } from "./lib/theme";

const [theme, setTheme] = useState<"light" | "dark">(readStoredTheme);
const liquid = useLiquidThemeReveal({
  theme,                                  // 受控主题
  onChange: setTheme,                     // 动画完成后提交
  applyDomTheme: applyThemeToDocument,    // 同任务内同步改 DOM → 零闪帧
  duration: 1050,                         // 可配置时长
  instant: false,                         // 性能模式/减少动效置 true
  audioEnabled: soundOn,                  // 内置音效（默认关）
  audioVolume: 0.45,
});

<button onClick={(e) => liquid.toggle(e)}>切换</button>
```

只想要动画、自己发声：

```tsx
const liquid = useLiquidThemeReveal({
  theme, onChange: setTheme,
  applyDomTheme,
  audio: {
    begin: (dir) => mySound.start(dir),
    update: (snap) => mySound.setIntensity(snap.stage, snap.flow),  // 必须自带阶段门控
    end: (r) => mySound.stop(r === "interrupted"),
  },
});
```

只想要引擎（无 React）：

```ts
import { createLiquidReveal } from "./components/liquid-theme";

const { handle } = createLiquidReveal(
  {
    direction: "darken",
    origin: { x: 1200, y: 20, radius: 14 },
    duration: 1050,
    backingColor: "#060609",
    clipId: "my-clip",
    onFrame: (snap) => { /* 每帧，别 setState */ },
    onEnd: (result) => { if (result === "complete") commitTheme(); },
  },
  { applyThemeScope: (el, t) => el.setAttribute("data-theme", t) },
);
handle.start();
```

### 换成别的主题命名（`day`/`night` 等）

本模块的 `LiquidTheme` 固定为 `"light" | "dark"`，因为液体方向与音色都依赖这个二值语义。
若你的项目用别的名字，只在两个回调里映射即可，模块内部不用改：

```ts
applyDomTheme: (t) => document.documentElement.setAttribute("data-theme", t === "dark" ? "night" : "day"),
// 并给 createLiquidReveal 传 applyThemeScope 做同样映射
```

## 设计约束（有意为之）

- **动画期间不动 React 主题状态**。底层保持旧主题、揭示层是新主题快照；动画走完时先同步提交
  主题再拆层，同一任务内完成 → 浏览器只画终态一帧。
- **单 rAF，每帧只写两个 `d` 属性**（clip 形 + 湿边形）。不 setState、不读布局、不建 DOM。
- **视觉快照而非双 React 树**。双树要复制业务状态（会分叉）或给每个组件加影子模式（等于重写）。
  克隆体不在 React 根内 → **零事件绑定**；`id`/`for`/`name`/`aria-*` 全部剥离 → 无重复 ID；
  `inert` + `aria-hidden` + `tabindex=-1` → 不进焦点与读屏顺序。
- **音效由真实进度驱动**。四个提示音挂在阶段边界（`STAGE_BOUNDS`），流水增益跟随几何算出的
  `flow`；没有一处固定 `setTimeout` 堆叠。

## 快照支持范围（不把静态快照说成能自动重绘）

| 内容 | 支持 | 说明 |
|---|---|---|
| `<img>` / CSS 背景图 | ✅ | 克隆 `src`，浏览器共享解码缓存 |
| SVG / 纯 CSS 图表 | ✅ | 连同样式一起克隆，按新主题变量重绘 |
| 表单控件 | ✅ | 复制当前 `value` / `checked` / `range` / `select` 状态 |
| 滚动位置 | ✅ | 克隆后按源容器逐个同步；resize / 滚动时再对 |
| `<canvas>` | ❌ | 克隆体是空壳（不复制像素） |
| `<video>` | ❌ | 克隆体不继续播放（冻结帧） |
| `<iframe>` | ❌ | 不复制内部文档 |
| Shadow DOM | ❌ | `cloneNode` 不带 shadow root |

本仓库 UI 无 canvas / video / iframe / shadow DOM（已 grep 确认），故不影响。
将来引入时：为它们提供自定义快照，或在切换期间临时隐藏。

## 降级与清理

| 情况 | 行为 |
|---|---|
| `prefers-reduced-motion: reduce` | 直接切主题；音效只播简短收尾 |
| 性能模式（`html.perf`） | 同上（CSS 也 `display:none` 掉舞台） |
| 不支持 `clip-path: url(#…)` | 直接切主题 |
| 拿不到按钮 rect | 直接切主题 |
| 页面在后台 | `start()` 内部直接 `finish("interrupted")`，不留层 |
| 重复点击 | 上一轮先 `finishNow()`（提交它承诺的主题），再开新的；不排队、不锁死 |
| 切视图 / 卸载 | `cancel()` / `destroy()`：取消 rAF、移除节点、解绑 resize/scroll/visibility |
| 音频不可用 / 解锁失败 | 全部 `try/catch` 静默；主题照常切换 |
| 静音 / 音量 0 / 进后台 | 立即 `stopFlow()`，不误播完成音 |

## 音效设计

| 阶段 | 声音 | 合成方式 |
|---|---|---|
| pour | 滴落/倾倒 | 带通噪声 1400→420Hz 扫频 + 180→88Hz 正弦"咕嘟" |
| impact | 水花 | 低通噪声爆发（5200→900Hz）+ 两滴短正弦 |
| rise | 流水 | 循环噪声 + 带通，增益/中心频率跟随 `flow` |
| settle | 收尾 | 两句柔和正弦：深色 392→294Hz / 浅色 392→523Hz |

方向影响音高：`darken` 系数 0.78（低沉），`lighten` 1.28（清亮）。
全部走 lowpass/bandpass，无 <400Hz 轰鸣、无 >6kHz 尖锐。默认**关闭**。

## 验证

```bash
# 纯静态/单测（零 CPU）
node tests/verify_liquid_theme_scope_2154.cjs    # 主题作用域：4 场景 + 反向对照 + 守卫覆盖率
node tests/verify_liquid_geometry_2154.cjs       # 时间轴/几何不变量/音效门控/多视口 + 反向对照

# 浏览器（需先起 dev server；一次只跑一个）
npx vite --port 5199 --strictPort
node tests/verify_liquid_theme_harness_2154.cjs  # 生产模块 + 真实 src/styles 端到端
node tests/verify_liquid_theme_browser_2154.cjs  # 演示页：滚动/sticky/表单/连点/降级

# 逐帧视觉留档
node tests/shoot_liquid_2154.cjs                 # → tests/artifacts/liquid_*.png
```

## 演示页

`design/liquid-theme-demo.html` —— 独立 HTML，双击即可打开（无需构建）。
用需求给定的调色板（`#F1F0ED` / `#141319` / `#060609` / `#F4F2F8` / `#E07A2E` / `#2F6BFF` / `#14A3A8`），
含卡片、表单、图表、可滚动区域与 sticky 顶栏，右上角 ◐ 触发切换，"音效"按钮开关声音。

生产模块联调页：`tests/harness/liquid-harness.html`（走 vite dev，用真实 `src/styles`）。
