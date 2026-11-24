/* [lc-2336] TVBox 配置导入（单链接）。
   从 WebrsPanel 的源管理区拆出的独立组件：本组件只管「填链接 → 预览 → 导入」的交互与
   结果展示，抓取与解析全在 Rust（`tvbox_import` 命令）。

   为什么要如实展示「跳过」：TVBox 生态绝大多数源是 csp/jar/drpy，需要 Android/JS 运行时，
   AniBox 没有。如果不说明，用户会以为「解析失败」；说明清楚才不会反复试。 */
import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type TvboxMacItem = {
  name: string;
  api: string;
  referer: string;
  installed: boolean;
};

export type TvboxImportResult = {
  url: string;
  rawBytes: number;
  stego: boolean;
  applied: boolean;
  stats: { maccms: number; csp: number; jar: number; js: number; other: number };
  maccms: TvboxMacItem[];
  lives: string[];
  iptvAdded: number;
  iptvOverflow: number;
  skipped: string[];
};

export function TvboxImport({ onImported }: { onImported: () => void | Promise<void> }) {
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [res, setRes] = useState<TvboxImportResult | null>(null);
  const [applyMsg, setApplyMsg] = useState("");

  const run = async (apply: boolean) => {
    const u = url.trim();
    if (!u || busy) return;
    setBusy(true);
    setErr("");
    setApplyMsg("");
    if (apply) setRes(null);
    try {
      const r = await invoke<TvboxImportResult>("tvbox_import", { url: u, apply });
      setRes(r);
      if (apply) {
        const parts: string[] = [];
        if (r.maccms.length) parts.push(`${r.maccms.length} 个采集源`);
        if (r.iptvAdded) parts.push(`${r.iptvAdded} 条直播源`);
        setApplyMsg(
          parts.length
            ? `已导入 ${parts.join(" 和 ")}。记得到源列表里勾选采集源。`
            : "这个配置里没有可导入的源（见下方跳过说明）。",
        );
        await onImported();
      }
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const previewable = !!(res && !res.applied);
  const hasImportable = previewable && (res!.maccms.length > 0 || res!.lives.length > 0);

  return (
    <div className="webrs-tvbox">
      <div className="webrs-mac-hint">
        粘贴 TVBox 配置链接（TVBox 里「接口」那一栏的地址）。会提取其中可用的苹果CMS 采集源与直播源；
        插件类源需要 Android 运行时，无法接入。
      </div>
      <div className="webrs-tvbox-row">
        <input
          className="webrs-mac-input"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") run(false);
          }}
          placeholder="http://example.com/tv"
          spellCheck={false}
        />
        <button className="webrs-go" disabled={busy || !url.trim()} onClick={() => run(false)}>
          {busy ? "解析中…" : "解析"}
        </button>
      </div>

      {err && <div className="webrs-err">{err}</div>}
      {applyMsg && <div className="webrs-hint">{applyMsg}</div>}

      {res && (
        <div className="webrs-tvbox-res">
          <div className="webrs-tvbox-sum">
            配置 {(res.rawBytes / 1024).toFixed(0)} KB
            {res.stego ? "（图片隐写解出）" : ""}　可导入 {res.maccms.length} 个采集源 ·{" "}
            {res.lives.length} 条直播源
          </div>

          {res.maccms.length > 0 && (
            <div className="webrs-tvbox-group">
              <div className="webrs-tvbox-title">苹果CMS 采集源</div>
              {res.maccms.map((m) => (
                <div key={m.api} className="webrs-tvbox-item">
                  <span className="webrs-tvbox-name">{m.name}</span>
                  {m.installed && <span className="webrs-tvbox-tag">已装</span>}
                  <span className="webrs-tvbox-url" title={m.api}>
                    {m.api}
                  </span>
                </div>
              ))}
            </div>
          )}

          {res.lives.length > 0 && (
            <div className="webrs-tvbox-group">
              <div className="webrs-tvbox-title">直播源</div>
              {res.lives.map((u) => (
                <div key={u} className="webrs-tvbox-item">
                  <span className="webrs-tvbox-url" title={u}>
                    {u}
                  </span>
                </div>
              ))}
            </div>
          )}

          {res.skipped.length > 0 && (
            <div className="webrs-tvbox-group">
              <div className="webrs-tvbox-title">已跳过</div>
              {res.skipped.map((s) => (
                <div key={s} className="webrs-tvbox-skip">
                  {s}
                </div>
              ))}
            </div>
          )}

          {previewable && hasImportable && (
            <div className="webrs-add-acts">
              <button className="webrs-go" disabled={busy} onClick={() => run(true)}>
                {busy ? "导入中…" : "导入这些源"}
              </button>
            </div>
          )}
          {res.applied && res.iptvOverflow > 0 && (
            <div className="webrs-tvbox-skip">
              直播源超出上限，{res.iptvOverflow} 条未加入（设置 → 通用 → 发现页直播源 最多 4 条）。
            </div>
          )}
        </div>
      )}
    </div>
  );
}
