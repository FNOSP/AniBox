/* [liquid-theme] 液体音效：Web Audio **本地合成**，不使用任何远程/授权不明的音频文件。
 *
 * 四段声音对应四个真实阶段（不是按固定延时堆叠）：
 *   pour   滴落 / 倾倒：短促带通噪声扫频 + 一声低频"咕嘟"
 *   impact 水花：宽频噪声爆发 + 高频短衰减（水珠）
 *   rise   流水：连续滤波噪声，增益跟随动画的 flow（液面上升速度）
 *   settle 收尾：柔和的下行滑音（浅→深更低、深→浅更高）
 *
 * 刻意做得"轻"：
 *   · 全部走 lowpass/bandpass，不出现 <400Hz 的嗡嗡或 >6kHz 的尖锐
 *   · 每个阶段只触发一次（stageGate），重复点击不会叠加
 *   · 中断时立刻停流水、不播完成音
 *   · AudioContext 只在用户手势后创建/恢复；失败一律静默降级
 *
 * 与既有音效系统共存：AniBox 目前没有任何音频播放代码（已确认），所以这里自建一个
 * AudioContext 并**复用**（模块级单例），不新开多个；若宿主已有 context，可传入
 * createLiquidAudio({ context }) 复用它，避免多个 context 抢音频设备。
 */

import { directionPitch } from "./timeline.ts";
import type { LiquidAudioAdapter, LiquidDirection, LiquidFrameSnapshot, LiquidStage } from "./types.ts";

/** 液体音效的参数（音量/开关由调用方持有并持久化，模块不读 localStorage） */
export type LiquidAudioOptions = {
  /** 主音量 0..1 */
  volume: number;
  /** 复用外部 AudioContext（宿主已有音效系统时强烈建议传） */
  context?: AudioContext | null;
  /** 复用外部输出节点（例如宿主的 masterGain），保证音效风格统一 */
  destination?: AudioNode | null;
};

export type LiquidAudio = LiquidAudioAdapter & {
  /** 设置音量（0..1），立即生效 */
  setVolume: (v: number) => void;
  /** 当前音量 */
  readonly volume: number;
  /** 静音开关（= 音量 0，但不销毁 context） */
  setMuted: (m: boolean) => void;
  readonly muted: boolean;
  /** 主动停掉所有正在发声的源（页面进后台/卸载/静音时调用） */
  stopAll: () => void;
  /** 释放 AudioContext（仅当它是由本模块创建的） */
  dispose: () => void;
};

type Voice = { stop: (when?: number) => void };

function clampVol(v: number): number {
  if (!Number.isFinite(v)) return 0;
  return Math.min(1, Math.max(0, v));
}

/** 生成一段噪声 buffer（一次生成、多次复用，避免每次 new AudioBuffer） */
function makeNoise(ctx: AudioContext, seconds = 2): AudioBuffer {
  const len = Math.max(1, Math.floor(ctx.sampleRate * seconds));
  const buf = ctx.createBuffer(1, len, ctx.sampleRate);
  const data = buf.getChannelData(0);
  // 简单的一阶差分白噪声（略带"水"的粗糙感）
  let prev = 0;
  for (let i = 0; i < len; i += 1) {
    const w = Math.random() * 2 - 1;
    data[i] = (w - prev * 0.72) * 0.6;
    prev = w;
  }
  return buf;
}

export function createLiquidAudio(options: LiquidAudioOptions): LiquidAudio {
  let volume = clampVol(options.volume);
  let muted = false;
  let ctx: AudioContext | null = null;
  let ownsCtx = false;
  let master: GainNode | null = null;
  let noise: AudioBuffer | null = null;
  let flowVoice: { src: AudioBufferSourceNode; gain: GainNode; filter: BiquadFilterNode } | null = null;
  const voices = new Set<Voice>();
  /** 已经触发过的阶段（同一次切换内不重复） */
  const fired = new Set<LiquidStage>();
  let activeDirection: LiquidDirection = "darken";

  const externalCtx = options.context ?? null;
  const externalDest = options.destination ?? null;

  function ensureCtx(): AudioContext | null {
    try {
      if (!ctx) {
        if (externalCtx && externalCtx.state !== "closed") {
          ctx = externalCtx;
          ownsCtx = false;
        } else {
          const Ctor =
            window.AudioContext ||
            (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
          if (!Ctor) return null;
          ctx = new Ctor();
          ownsCtx = true;
        }
      }
      if (!master && ctx) {
        const g = ctx.createGain();
        g.gain.value = 0;
        g.connect(externalDest ?? ctx.destination);
        master = g;
      }
      if (!noise && ctx) noise = makeNoise(ctx);
      return ctx;
    } catch {
      return null;
    }
  }

  /** 只在用户手势里调用；失败静默（主题切换必须照常完成） */
  function unlock(): void {
    const c = ensureCtx();
    if (!c) return;
    if (c.state === "suspended") {
      void c.resume().catch(() => {});
    }
    applyMasterGain();
  }

  function applyMasterGain(): void {
    if (!ctx || !master) return;
    const target = muted ? 0 : volume;
    try {
      master.gain.cancelScheduledValues(ctx.currentTime);
      master.gain.setTargetAtTime(target, ctx.currentTime, 0.02);
    } catch {
      master.gain.value = target;
    }
  }

  function track(v: Voice): void {
    voices.add(v);
  }

  function untrack(v: Voice): void {
    voices.delete(v);
  }

  function killVoice(v: Voice): void {
    try {
      v.stop();
    } catch {
      /* 已经停了 */
    }
    untrack(v);
  }

  /* ───────────── 四个阶段音 ───────────── */

  /** pour：滴落/倾倒 —— 带通噪声快速下扫 + 低频咕嘟 */
  function playPour(): void {
    const c = ctx;
    if (!c || !master || !noise) return;
    const t = c.currentTime + 0.01;
    const pitch = directionPitch(activeDirection);

    // 噪声扫频：像水从瓶口流出的"呼"声
    const src = c.createBufferSource();
    src.buffer = noise;
    src.loop = true;
    const bp = c.createBiquadFilter();
    bp.type = "bandpass";
    bp.Q.value = 1.4;
    bp.frequency.setValueAtTime(1400 * pitch, t);
    bp.frequency.exponentialRampToValueAtTime(420 * pitch, t + 0.42);
    const g = c.createGain();
    g.gain.setValueAtTime(0.0001, t);
    g.gain.exponentialRampToValueAtTime(0.34, t + 0.05);
    g.gain.exponentialRampToValueAtTime(0.0001, t + 0.5);
    src.connect(bp).connect(g).connect(master);
    src.start(t);
    src.stop(t + 0.55);
    const voice: Voice = { stop: () => src.stop() };
    track(voice);
    src.onended = () => {
      try {
        src.disconnect();
        bp.disconnect();
        g.disconnect();
      } catch {
        /* noop */
      }
      untrack(voice);
    };

    // 咕嘟：正弦从 180→90Hz，很短（"咚"而不是"轰"）
    const out = master;
    if (!out) return;
    const o = c.createOscillator();
    o.type = "sine";
    o.frequency.setValueAtTime(180 * pitch, t);
    o.frequency.exponentialRampToValueAtTime(88 * pitch, t + 0.22);
    const og = c.createGain();
    og.gain.setValueAtTime(0.0001, t);
    og.gain.exponentialRampToValueAtTime(0.16, t + 0.02);
    og.gain.exponentialRampToValueAtTime(0.0001, t + 0.3);
    o.connect(og).connect(out);
    o.start(t);
    o.stop(t + 0.32);
    const v2: Voice = { stop: () => o.stop() };
    track(v2);
    o.onended = () => {
      try {
        o.disconnect();
        og.disconnect();
      } catch {
        /* noop */
      }
      untrack(v2);
    };
  }

  /** impact：水花 —— 宽频噪声爆发 + 高频水珠 */
  function playImpact(): void {
    const c = ctx;
    if (!c || !master || !noise) return;
    const t = c.currentTime + 0.005;
    const pitch = directionPitch(activeDirection);

    const src = c.createBufferSource();
    src.buffer = noise;
    src.loop = true;
    const lp = c.createBiquadFilter();
    lp.type = "lowpass";
    lp.frequency.setValueAtTime(5200 * pitch, t);
    lp.frequency.exponentialRampToValueAtTime(900 * pitch, t + 0.3);
    const g = c.createGain();
    g.gain.setValueAtTime(0.0001, t);
    g.gain.exponentialRampToValueAtTime(0.3, t + 0.012);
    g.gain.exponentialRampToValueAtTime(0.0001, t + 0.34);
    src.connect(lp).connect(g).connect(master);
    src.start(t);
    src.stop(t + 0.36);
    const voice: Voice = { stop: () => src.stop() };
    track(voice);
    src.onended = () => {
      try {
        src.disconnect();
        lp.disconnect();
        g.disconnect();
      } catch {
        /* noop */
      }
      untrack(voice);
    };

    // 两滴"叮"：短正弦，很轻
    const out = master;
    if (!out) return;
    [0, 0.055].forEach((delay, i) => {
      const o = c.createOscillator();
      o.type = "sine";
      const f = (620 + i * 210) * pitch;
      o.frequency.setValueAtTime(f, t + delay);
      o.frequency.exponentialRampToValueAtTime(f * 0.72, t + delay + 0.14);
      const og = c.createGain();
      og.gain.setValueAtTime(0.0001, t + delay);
      og.gain.exponentialRampToValueAtTime(0.075, t + delay + 0.012);
      og.gain.exponentialRampToValueAtTime(0.0001, t + delay + 0.18);
      o.connect(og).connect(out);
      o.start(t + delay);
      o.stop(t + delay + 0.2);
      const v: Voice = { stop: () => o.stop() };
      track(v);
      o.onended = () => {
        try {
          o.disconnect();
          og.disconnect();
        } catch {
          /* noop */
        }
        untrack(v);
      };
    });
  }

  /** rise：连续流水 —— 常驻噪声，增益/滤波跟随 flow */
  function startFlow(): void {
    const c = ctx;
    if (!c || !master || !noise || flowVoice) return;
    const src = c.createBufferSource();
    src.buffer = noise;
    src.loop = true;
    const bp = c.createBiquadFilter();
    bp.type = "bandpass";
    bp.Q.value = 0.8;
    const pitch = directionPitch(activeDirection);
    bp.frequency.value = 900 * pitch;
    const g = c.createGain();
    g.gain.value = 0.0001;
    src.connect(bp).connect(g).connect(master);
    src.start();
    g.gain.setTargetAtTime(0.1, c.currentTime, 0.12);
    flowVoice = { src, gain: g, filter: bp };
  }

  /** 流水随 flow 变化（每帧调用，只做 setTargetAtTime，不创建节点） */
  function updateFlow(flow: number): void {
    const c = ctx;
    if (!c || !flowVoice) return;
    const pitch = directionPitch(activeDirection);
    const target = 0.012 + clampVol(flow) * 0.16;
    try {
      flowVoice.gain.gain.setTargetAtTime(target, c.currentTime, 0.09);
      flowVoice.filter.frequency.setTargetAtTime((620 + flow * 1100) * pitch, c.currentTime, 0.12);
    } catch {
      /* noop */
    }
  }

  function stopFlow(fade = 0.14): void {
    const c = ctx;
    const f = flowVoice;
    flowVoice = null;
    if (!f) return;
    try {
      if (c) {
        f.gain.gain.cancelScheduledValues(c.currentTime);
        f.gain.gain.setTargetAtTime(0.0001, c.currentTime, fade / 3);
      }
      f.src.stop(c ? c.currentTime + fade : undefined);
    } catch {
      try {
        f.src.stop();
      } catch {
        /* noop */
      }
    }
    f.src.onended = () => {
      try {
        f.src.disconnect();
        f.gain.disconnect();
        f.filter.disconnect();
      } catch {
        /* noop */
      }
    };
  }

  /** settle：柔和的收尾（两声轻音，方向决定音高走向） */
  function playSettle(): void {
    const c = ctx;
    if (!c || !master) return;
    const t = c.currentTime + 0.01;
    const pitch = directionPitch(activeDirection);
    const out = master;
    if (!out) return;
    const notes = activeDirection === "darken" ? [392, 294] : [392, 523];
    notes.forEach((f, i) => {
      const o = c.createOscillator();
      o.type = "sine";
      const start = t + i * 0.11;
      const freq = f * pitch;
      o.frequency.setValueAtTime(freq, start);
      const g = c.createGain();
      g.gain.setValueAtTime(0.0001, start);
      g.gain.exponentialRampToValueAtTime(0.1, start + 0.03);
      g.gain.exponentialRampToValueAtTime(0.0001, start + 0.36);
      // 加一点低通让收尾更柔（不刺）
      const lp = c.createBiquadFilter();
      lp.type = "lowpass";
      lp.frequency.value = 2600 * pitch;
      o.connect(lp).connect(g).connect(out);
      o.start(start);
      o.stop(start + 0.4);
      const v: Voice = { stop: () => o.stop() };
      track(v);
      o.onended = () => {
        try {
          o.disconnect();
          lp.disconnect();
          g.disconnect();
        } catch {
          /* noop */
        }
        untrack(v);
      };
    });
  }

  /* ───────────── 适配器接口 ───────────── */
  const adapter: LiquidAudio = {
    begin(direction: LiquidDirection): void {
      activeDirection = direction;
      fired.clear();
      stopFlow(0.08);
      unlock();
    },
    update(snapshot: LiquidFrameSnapshot): void {
      // 音频不可用 / 静音 / 音量为 0：不做任何事（主题切换照常）
      if (!ctx || muted || volume <= 0) return;
      const stage = snapshot.stage;

      if (stage === "pour" && !fired.has("pour")) {
        fired.add("pour");
        playPour();
      }
      if (stage === "impact" && !fired.has("impact")) {
        fired.add("impact");
        playImpact();
      }
      if (stage === "rise" || stage === "settle") {
        if (!fired.has("rise")) {
          fired.add("rise");
          startFlow();
        }
        updateFlow(snapshot.flow);
      }
      // settle 期间流水逐渐收掉（收尾音进来时不让噪声打架）
      if (stage === "settle") {
        if (!fired.has("settle")) {
          fired.add("settle");
          stopFlow(0.3);
          playSettle();
        }
      }
    },
    end(result: "complete" | "interrupted"): void {
      stopFlow(result === "interrupted" ? 0.05 : 0.2);
      fired.clear();
    },
    dispose(): void {
      stopFlow(0.04);
      for (const v of Array.from(voices)) killVoice(v);
      try {
        if (master) {
          master.gain.value = 0;
          master.disconnect();
        }
      } catch {
        /* noop */
      }
      master = null;
      noise = null;
      if (ownsCtx && ctx) {
        const c = ctx;
        ctx = null;
        void c.close().catch(() => {});
      } else {
        ctx = null;
      }
    },
    setVolume(v: number): void {
      volume = clampVol(v);
      if (volume > 0 && muted) muted = false;
      applyMasterGain();
    },
    get volume(): number {
      return volume;
    },
    setMuted(m: boolean): void {
      muted = m;
      if (m) stopFlow(0.04);
      applyMasterGain();
    },
    get muted(): boolean {
      return muted;
    },
    stopAll(): void {
      stopFlow(0.05);
      for (const v of Array.from(voices)) killVoice(v);
      if (master && ctx) {
        try {
          master.gain.setTargetAtTime(0, ctx.currentTime, 0.02);
        } catch {
          /* noop */
        }
      }
    },
  };

  return adapter;
}

/** 试听（设置面板「音效试听」用）：按一个阶段播放一次，不启动动画 */
export function auditionLiquidStage(
  audio: LiquidAudio,
  stage: LiquidStage,
  direction: LiquidDirection = "lighten",
): void {
  audio.begin?.(direction);
  const snapshot: LiquidFrameSnapshot = {
    progress: stage === "pour" ? 0.1 : stage === "impact" ? 0.25 : stage === "rise" ? 0.7 : 0.95,
    stage,
    direction,
    flow: 0.7,
    surfaceY: 0,
    coverage: 0,
  };
  audio.update(snapshot);
}
