/* [lc-2064] 版本更新说明弹窗（每个版本只弹一次）。
 *
 * 外壳复用仓库既有的 .ddet-mask / .ddet（与设置面板、UpdatePrompt 同款），
 * 内部排版走 49-changelog.css 的 clg-*。portal 到 body，不被 .app-shell
 * 的圆角/溢出与视图转场上下文裁剪。
 *
 * 版本号来源：`getVersion()`（Tauri 运行时，读 tauri.conf.json = 安装包真实版本）。
 * 组件里**没有任何版本常量** —— 发版只需在 lib/changelog.ts 的 NOTES 里加一条。
 *
 * 与 UpdatePrompt 的区别见 lib/changelog.ts 头部注释。
 */
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { getVersion } from "@tauri-apps/api/app";
import { FlaskConical as IcoFlask, Sparkles as IcoSparkles } from "lucide-react";
import {
  changelogFor,
  changelogSeen,
  markChangelogSeen,
  type ChangelogEntry,
} from "../../lib/changelog";

type Entry = ChangelogEntry & { version: string };

export default function ChangelogPrompt() {
  const [entry, setEntry] = useState<Entry | null>(null);
  const [closing, setClosing] = useState(false);

  useEffect(() => {
    let alive = true;
    // 版本号从 Tauri 取（与 tauri.conf.json 一致）；取不到就不弹 —— 宁可不出，
    // 也不要用一个猜的版本号把「已看过」记错（那会让真正的说明永远弹不出来）。
    getVersion()
      .then((v) => {
        if (!alive) return;
        if (changelogSeen(v)) return;
        const e = changelogFor(v);
        if (!e) {
          // 这一版没写说明 → 直接标记为已看过，避免每次启动都跑一遍无意义查询
          markChangelogSeen(v);
          return;
        }
        setEntry(e);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  const close = () => {
    if (!entry) return;
    markChangelogSeen(entry.version);
    // 退场动画（160ms）走完再卸载，与页面退场同一套节奏
    setClosing(true);
    window.setTimeout(() => setEntry(null), 160);
  };

  if (!entry) return null;

  return createPortal(
    <div
      className={`ddet-mask${closing ? " clg-closing" : ""}`}
      onClick={close}
      role="dialog"
      aria-modal="true"
      aria-label="更新说明"
    >
      <div className="ddet upx-modal clg-modal" onClick={(e) => e.stopPropagation()}>
        <button className="ddet-close" aria-label="关闭" onClick={close}>
          ×
        </button>

        <div className="clg-head">
          <div className="clg-badge">
            <IcoSparkles size={15} strokeWidth={2.2} />
            <span>v{entry.version} 更新</span>
          </div>
          <h2 className="clg-title">{entry.title}</h2>
        </div>

        <ul className="clg-list">
          {entry.items.map((t: string, i: number) => (
            <li key={i}>{t}</li>
          ))}
        </ul>

        {entry.beta && (
          <div className="clg-beta">
            <IcoFlask size={14} strokeWidth={2.2} />
            <span>{entry.beta}</span>
          </div>
        )}

        {entry.tip && <div className="clg-tip">{entry.tip}</div>}

        <div className="clg-foot">
          <button className="set-btn primary clg-ok" onClick={close} autoFocus>
            知道了
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
