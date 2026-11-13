/* [liquid-theme] 液体主题切换 —— 公共类型。
 *
 * 设计边界（有意为之）：
 *   · 核心（geometry / snapshot / LiquidReveal）**不依赖 React**，只碰 DOM 与 SVG。
 *     React 层只有 useLiquidThemeReveal 一个订阅点，且它**不**在每帧 setState ——
 *     逐帧只改 <path d> / 描边 d 两个属性，组件树一帧都不重渲染。
 *   · 主题用字面量联合 "light" | "dark"：本模块的液体方向（下沉/清亮）与音色
 *     都依赖这个二值语义，泛化成任意字符串只会让调用方多写映射还占不到便宜。
 *     若项目用的是别的命名（如 "day"/"night"），在调用处映射即可（见 README）。
 */

/** 主题名（受控状态由调用方持有，本模块只提交结果） */
export type LiquidTheme = "light" | "dark";

/** 液体方向：由浅入深 = darken（音色低沉），由深入浅 = lighten（音色清亮） */
export type LiquidDirection = "darken" | "lighten";

/** 动画阶段。音效的每一次触发都挂在这些阶段的**边界**上，不靠固定延时堆叠。 */
export type LiquidStage = "idle" | "pour" | "impact" | "pool" | "rise" | "settle" | "done";

/** 每帧推给"外部消费者"（音效 / 调试探针）的快照。刻意做成不可变值对象：
 *  它不引用 DOM，便于在 rAF 里零分配地传给音效适配器。 */
export type LiquidFrameSnapshot = {
  /** 0..1 的归一化进度（由真实时钟换算，不是帧计数） */
  progress: number;
  stage: LiquidStage;
  direction: LiquidDirection;
  /** 0..1 的"水流量"：由液面上升速度与溅起能量推导，驱动连续流水声的增益 */
  flow: number;
  /** 液面 y（视口坐标；< 0 表示已漫过顶部） */
  surfaceY: number;
  /** 覆盖率 0..1（用于测试断言与降级判断） */
  coverage: number;
};

/** 音效适配器：模块**只**通过这个接口发声。
 *  调用方可以完全不接入（只做液体动画），也可以接自己的音效系统。 */
export type LiquidAudioAdapter = {
  /** 一次切换开始（用户手势内调用，适合在这里初始化/恢复 AudioContext） */
  begin?: (direction: LiquidDirection) => void;
  /** 每帧调用：实现方必须自己保证幂等（同阶段不重复触发） */
  update: (snapshot: LiquidFrameSnapshot) => void;
  /** 结束。complete = 正常走完（允许收尾声）；interrupted = 中断（不许播完成提示） */
  end?: (result: "complete" | "interrupted") => void;
  /** 组件卸载 / 模块销毁 */
  dispose?: () => void;
};

/** 液体起点：按钮中心的视口坐标 + 按钮的等效半径（决定液柱粗细与出口大小） */
export type LiquidOrigin = { x: number; y: number; radius: number };

/** 可复用的"起点来源"：元素、鼠标/指针事件（React 合成事件也有 currentTarget）或裸坐标 */
export type LiquidOriginSource =
  | LiquidOrigin
  | Element
  | { currentTarget: Element | null }
  | null
  | undefined;

/** 核心引擎的启动配置 */
export type LiquidRevealConfig = {
  /** 方向（决定音色与默认背面底色） */
  direction: LiquidDirection;
  /** 起点 */
  origin: LiquidOrigin;
  /** 动画时长（ms）。<= 0 表示立即完成（降级路径） */
  duration: number;
  /** 克隆源（默认 document.getElementById("root")）；找不到则回退到 body */
  snapshotTarget?: () => Element | null;
  /** 揭示层的不透明底（液体盖住后这层必须是不透的，否则会透出旧主题） */
  backingColor: string;
  /** clipPath 的 id（同文档内唯一） */
  clipId: string;
  /** 是否吞掉动画期间的指针事件（默认 true：避免底层被点而克隆层过期） */
  blockInteraction?: boolean;
  /** 每帧回调（音效/探针）；不要在这里 setState */
  onFrame?: (snapshot: LiquidFrameSnapshot) => void;
  /** 阶段变化回调（每个阶段各一次） */
  onStage?: (stage: LiquidStage, snapshot: LiquidFrameSnapshot) => void;
  /** 结束回调：reason=complete 正常走完；interrupted 被中断 */
  onEnd?: (result: "complete" | "interrupted") => void;
};

/** React 侧选项 */
export type UseLiquidThemeOptions = {
  /** 当前主题（受控） */
  theme: LiquidTheme;
  /** 主题提交回调（动画结束 / 中断各调用一次，永不重复） */
  onChange: (next: LiquidTheme) => void;
  /** 提交主题时**同步**改 DOM 的钩子（例如 html.dark）。给了它才能做到零闪帧：
   *  本模块会在移除揭示层的同一个任务里先调用它，React 的 effect 随后重复设置同值无害。 */
  applyDomTheme?: (next: LiquidTheme) => void;
  /** 动画时长 ms，默认 1050 */
  duration?: number;
  /** 跳过液体动画，直接切主题（性能模式 / 减少动效 / 不支持 clip-path） */
  instant?: boolean;
  /** 音效适配器；null = 不发声 */
  audio?: LiquidAudioAdapter | null;
  /** 克隆源，默认 #root */
  snapshotTarget?: () => Element | null;
  /** 揭示层不透明底；默认取 LIQUID_PALETTE 的纸白/近黑 */
  backing?: Partial<Record<LiquidTheme, string>>;
  /** 阶段回调（可选，用于 UI 提示） */
  onStage?: (stage: LiquidStage, snapshot: LiquidFrameSnapshot) => void;
};
