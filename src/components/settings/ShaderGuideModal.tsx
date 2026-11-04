/* [lc-2009] 着色器速查弹窗 —— 版式照用户给的折叠块：一级 4 项（动漫 / 真人 /
   滤镜系列速查 / 单个着色器开关），原生 <details> 折叠，表格内组合/单开关可点。
   数据在 src/lib/shaderGuide.ts（新版 UI 同步时组件与数据可整体搬走）。 */
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import {
  ANIME_GUIDE,
  GUIDE_TIPS,
  LIVE_GUIDE,
  SERIES_GROUPS,
  SWITCH_GROUP,
  type GuideRow,
} from "../../lib/shaderGuide";

const PERF_COLOR: Record<string, string> = {
  低: "var(--green-9, #30a46c)",
  中: "var(--blue-9, #0091ff)",
  高: "var(--orange-9, #f76b15)",
  极高: "var(--red-9, #e5484d)",
};

function perfColor(p: string): string {
  if (p.includes("极高")) return PERF_COLOR["极高"];
  if (p.includes("高")) return PERF_COLOR["高"];
  if (p.includes("中")) return PERF_COLOR["中"];
  if (p.includes("低")) return PERF_COLOR["低"];
  return "var(--gray-9, #8b8d98)";
}

type Props = {
  open: boolean;
  onClose: () => void;
  /** 应用组合预设（替换式，走 shader_settings_set） */
  onApply: (key: string) => Promise<void>;
  /** 当前持久化链（用于单开关高亮判断） */
  chain: string;
  /** 单开关 toggle 完成（含新链）回调 */
  onChainChange: (chain: string) => void;
};

export function ShaderGuideModal({ open, onClose, onApply, chain, onChainChange }: Props) {
  const [busy, setBusy] = useState<string | null>(null);
  const [tip, setTip] = useState("");
  const boxRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;

  const chainParts = chain ? chain.split(";").filter(Boolean) : [];

  const applyCombo = async (key: string) => {
    setBusy(key);
    setTip("");
    try {
      await onApply(key);
      setTip("已应用组合");
    } catch (e) {
      setTip(String(e));
    } finally {
      setBusy(null);
    }
  };

  const toggleSwitch = async (key: string) => {
    setBusy(key);
    setTip("");
    try {
      const r = await invoke<{ removed: boolean; chain: string }>("shader_switch_toggle", { preset: key });
      onChainChange(r.chain);
      setTip(r.removed ? "已从当前链移除" : "已叠加到当前链（再点一次移除）");
    } catch (e) {
      setTip(String(e));
    } finally {
      setBusy(null);
    }
  };

  const chipActive = (key: string) => chainParts.some((p) => p.includes(key));

  const renderChips = (row: GuideRow, toggle: boolean) => (
    row.chips?.map((c) => {
      const active = toggle && chipActive(c.key);
      return (
        <button
          key={c.key}
          type="button"
          disabled={busy !== null}
          className={`sg-chip${active ? " on" : ""}`}
          onClick={() => void (toggle ? toggleSwitch(c.key) : applyCombo(c.key))}
        >
          {busy === c.key ? "…" : active ? "✓ " : ""}{c.label}
        </button>
      );
    }) ?? null
  );

  const table = (rows: GuideRow[], toggle: boolean) => (
    <table className="sg-table">
      <thead>
        <tr>
          <th style={{ width: toggle ? "30%" : "26%" }}>{toggle ? "菜单名称" : "片源 → 输出"}</th>
          {toggle ? <th style={{ width: "44%" }}>Shader / 功能</th> : <th style={{ width: "44%" }}>推荐组合</th>}
          <th style={{ width: "12%" }}>性能</th>
          <th style={{ width: "14%" }}>{toggle ? "开关" : "应用"}</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((r, i) => (
          <tr key={i}>
            <td className="sg-scene">
              {r.scene}
              {r.note ? <div className="sg-note">{r.note}</div> : null}
            </td>
            <td className="sg-func">{r.func ?? renderChips(r, false) ?? "—"}</td>
            <td><span className="sg-perf" style={{ color: perfColor(r.perf) }}>{r.perf}</span></td>
            <td>{r.chips ? renderChips(r, toggle) : <span className="sg-note">—</span>}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );

  return createPortal(
    <div className="sg-overlay" onClick={onClose}>
      <div
        ref={boxRef}
        className="sg-box"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-label="滤镜速查"
      >
        <div className="sg-head">
          <span className="sg-title">滤镜速查</span>
          <button type="button" className="sg-close" onClick={onClose} aria-label="关闭">✕</button>
        </div>

        <div className="sg-body">
          <details className="sg-sec" open>
            <summary><b>1. 动漫</b><span className="sg-sum">4K 片源无需超分；Anime4K 针对动漫优化</span></summary>
            {table(ANIME_GUIDE.rows, false)}
          </details>

          <details className="sg-sec">
            <summary><b>2. 真人</b><span className="sg-sum">不推荐 Anime4K，易油画感、皮肤纹理丢失</span></summary>
            {table(LIVE_GUIDE.rows, false)}
          </details>

          <details className="sg-sec">
            <summary><b>3. 滤镜系列速查</b><span className="sg-sum">新版 ArtCNN/CuNNy 需手动开色度；Anime4K 真人慎用</span></summary>
            {SERIES_GROUPS.map((g) => (
              <details key={g.id} className="sg-sub">
                <summary>{g.title}<span className="sg-sum">{g.summary}</span></summary>
                {table(g.rows, false)}
              </details>
            ))}
          </details>

          <details className="sg-sec">
            <summary><b>4. 单个着色器开关</b><span className="sg-sum">点击叠加到当前组合 / 再点移除；CHROMA 需手动开才有色度放大</span></summary>
            {table(SWITCH_GROUP.rows, true)}
          </details>

          <div className="sg-tips">
            {GUIDE_TIPS.map((t, i) => (
              <div key={i} className="sg-tip">{t}</div>
            ))}
          </div>
          {tip && <div className="sg-status">{tip}</div>}
        </div>
      </div>
    </div>,
    document.body,
  );
}
