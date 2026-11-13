/* [liquid-theme] 偏好持久化：音效开关与音量。
 *  默认关闭（需求要求）；读写 localStorage，键名带模块前缀，互不干扰。 */

const KEY = "anibox.liquidTheme.audio";

export type LiquidAudioPrefs = {
  enabled: boolean;
  volume: number;
};

export const DEFAULT_LIQUID_AUDIO_PREFS: LiquidAudioPrefs = {
  enabled: false,
  volume: 0.45,
};

export function loadLiquidAudioPrefs(storage?: Storage): LiquidAudioPrefs {
  const s = storage ?? (typeof localStorage !== "undefined" ? localStorage : null);
  if (!s) return { ...DEFAULT_LIQUID_AUDIO_PREFS };
  try {
    const raw = s.getItem(KEY);
    if (!raw) return { ...DEFAULT_LIQUID_AUDIO_PREFS };
    const parsed = JSON.parse(raw) as Partial<LiquidAudioPrefs>;
    const volume = typeof parsed.volume === "number" ? Math.min(1, Math.max(0, parsed.volume)) : DEFAULT_LIQUID_AUDIO_PREFS.volume;
    return { enabled: parsed.enabled === true, volume };
  } catch {
    return { ...DEFAULT_LIQUID_AUDIO_PREFS };
  }
}

export function saveLiquidAudioPrefs(p: LiquidAudioPrefs, storage?: Storage): void {
  const s = storage ?? (typeof localStorage !== "undefined" ? localStorage : null);
  if (!s) return;
  try {
    s.setItem(KEY, JSON.stringify({ enabled: p.enabled === true, volume: Math.min(1, Math.max(0, p.volume)) }));
  } catch {
    /* 隐私模式/配额满：忽略 */
  }
}
