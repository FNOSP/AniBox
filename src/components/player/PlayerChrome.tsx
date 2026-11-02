/* [lc-1915] mpv_overlay 专用控制层（Polaris "MPV Overlay" 窗同角色）：
 *
 * 播放期间可见的是这个独立窗口，mpv 满屏压在其上、z 序紧贴其后。本层默认**全窗
 * 鼠标穿透**（点击/滚轮全部落到 mpv），Rust 把 mpv 看到的光标位置经 overlay-mouse
 * 转发回来，这里做命中测试：悬停到 [data-chrome] 元素上才关穿透、让按钮可点。
 * 这是 Polaris set_ignore_cursor_events 通道的复刻。
 *
 * [lc-1918] 加载态：mpv 出画前显示 转圈 + 片名（mpv-load 事件带 title），
 * loaded 后淡出、切集（start）重现 —— 主窗已隐藏，等待期不能是无反馈黑屏。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ArrowLeft as IcoArrowLeft } from "lucide-react";

type MpvLoad = { phase?: string; title?: string };

export function PlayerChrome() {
  const ignoreRef = useRef(true);
  const [loading, setLoading] = useState(true);
  const [title, setTitle] = useState("");

  const setIgnore = useCallback((v: boolean) => {
    if (ignoreRef.current === v) return;
    ignoreRef.current = v;
    void invoke("player_chrome_ignore", { ignore: v }).catch(() => {});
  }, []);

  useEffect(() => {
    let un: (() => void) | undefined;
    listen<{ x: number; y: number }>("overlay-mouse", (e) => {
      const { x, y } = e.payload;
      if (x < 0) {
        // 光标离开视频区：恢复全穿透
        setIgnore(true);
        return;
      }
      const dpr = window.devicePixelRatio || 1;
      const el = document.elementFromPoint(x / dpr, y / dpr);
      const hit = !!(el && el.closest("[data-chrome]"));
      setIgnore(!hit);
    })
      .then((f) => {
        un = f;
      })
      .catch(() => {});
    return () => un?.();
  }, [setIgnore]);

  // 加载态（与主窗 PlayerView 同事件源）：loaded → 淡出；start（起播/切集）→ 重现
  useEffect(() => {
    let un: (() => void) | undefined;
    listen<MpvLoad>("mpv-load", (e) => {
      const phase = e.payload?.phase;
      if (e.payload?.title) setTitle(e.payload.title);
      if (phase === "loaded") setLoading(false);
      else if (phase === "start") setLoading(true);
    })
      .then((f) => {
        un = f;
      })
      .catch(() => {});
    return () => un?.();
  }, []);

  return (
    <div className="player-chrome">
      <button
        type="button"
        data-chrome
        className="chrome-back"
        title="返回"
        aria-label="返回"
        onClick={() => void invoke("player_embed_leave", { stop: true }).catch(() => {})}
      >
        <IcoArrowLeft size={18} strokeWidth={1.8} />
      </button>
      <div className={`chrome-opening${loading ? "" : " gone"}`} aria-hidden={!loading}>
        <span className="player-spin" />
        <span className="chrome-opening-title">{title || "正在打开"}</span>
      </div>
    </div>
  );
}
