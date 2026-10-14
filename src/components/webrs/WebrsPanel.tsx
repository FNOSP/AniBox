/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { createPortal } from "react-dom";
import { useDelayedUnmount } from "../../hooks/useDelayedUnmount";

export type WebrsSearchItem = { name: string; url: string };

export type WebrsRoad = { name: string; index: number; episodes: WebrsSearchItem[] };

export type WebrsGroup = { name: string; status: "ok" | "empty" | "error"; error: string; items: WebrsSearchItem[] };

export type WebrsRuleInfo = { name: string; mode?: string };


export const WEBRS_ENABLED_KEY = "anibox.webrs.enabledSources";

/** 已勾选源名单（localStorage 持久化）。null = 从未改过 → 视为全启用。 */

export function loadEnabled(): string[] | null {
  try {
    const raw = localStorage.getItem(WEBRS_ENABLED_KEY);
    if (raw == null) return null;
    const v = JSON.parse(raw);
    return Array.isArray(v) ? v.filter((x) => typeof x === "string") : [];
  } catch {
    return null;
  }
}


export function WebrsPanel({ title }: { title: string }) {
  const [groups, setGroups] = useState<WebrsGroup[] | null>(null); // null = 尚未搜索
  const [kw, setKw] = useState(title);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  // 源管理
  const [srcs, setSrcs] = useState<WebrsRuleInfo[]>([]);
  const [enabled, setEnabled] = useState<string[] | null>(null); // null = 全启用
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [addOpen, setAddOpen] = useState(false);
  const [addText, setAddText] = useState("");
  const [addErr, setAddErr] = useState("");
  // [lc-1503] 一键测试源：对全部源各发一次真实搜索，行内显示 ✓ n / 无结果 / ✗
  const [tests, setTests] = useState<Record<string, { status: string; count: number; error: string }>>({});
  const [testing, setTesting] = useState(false);
  // 二级视图：选集
  const [sel, setSel] = useState<{ rule: string; item: WebrsSearchItem } | null>(null);
  const [roads, setRoads] = useState<WebrsRoad[]>([]);
  const [roadIdx, setRoadIdx] = useState(0);
  const [subBusy, setSubBusy] = useState("");

  const isEnabled = useCallback(
    (name: string) => enabled == null || enabled.includes(name),
    [enabled],
  );
  const enabledNames = useMemo(() => srcs.filter((r) => isEnabled(r.name)).map((r) => r.name), [srcs, isEnabled]);

  const refreshRules = useCallback(async () => {
    try {
      const r = await invoke<WebrsRuleInfo[]>("webrs_rules");
      setSrcs(r);
      return r;
    } catch (e) {
      setErr(String(e));
      return [];
    }
  }, []);

  // [lc-1508] 引导提示（中性样式，区别于真错误 err）：如"还没勾选任何源"
  const [srcHint, setSrcHint] = useState("");
  // [lc-1513] 二级视图（选集）返回时先播退出动画再卸载
  const [subMounted, subClosing] = useDelayedUnmount(!!sel);
  const doSearch = useCallback(
    async (k: string) => {
      const key = k.trim();
      if (!key) return;
      const picked = srcs.filter((r) => isEnabled(r.name)).map((r) => r.name);
      if (picked.length === 0) {
        setSrcHint("还没有勾选任何源，点右侧 ⚙ 选择要用的源");
        return;
      }
      setSrcHint("");
      setBusy(true);
      setErr("");
      setSel(null);
      setRoads([]);
      try {
        const r = await invoke<WebrsGroup[]>("webrs_search_multi", { keyword: key, rules: picked });
        setGroups(r);
        const first = r.find((g) => g.status === "ok");
        setExpanded(first ? { [first.name]: true } : {});
      } catch (e) {
        setErr(String(e));
        setGroups([]);
      } finally {
        setBusy(false);
      }
    },
    [srcs, isEnabled],
  );

  // 进面板：拉源列表 + 自动按标题搜一次（只搜勾选的）
  const autoDone = useRef(false);
  useEffect(() => {
    if (autoDone.current) return;
    autoDone.current = true;
    setEnabled(loadEnabled());
    refreshRules().then((r) => {
      const en = loadEnabled();
      const picked = r.filter((x) => en == null || en.includes(x.name)).map((x) => x.name);
      if (r.length > 0 && picked.length > 0) doSearch(title);
      else setSrcHint(r.length > 0 ? "还没有勾选任何源，点右侧 ⚙ 选择要用的源" : "还没有可用源，点 ⚙ 添加规则");
    });
  }, []);

  const toggleSource = (name: string) => {
    const cur = srcs.filter((r) => isEnabled(r.name)).map((r) => r.name);
    const next = cur.includes(name) ? cur.filter((x) => x !== name) : [...cur, name];
    setEnabled(next);
    localStorage.setItem(WEBRS_ENABLED_KEY, JSON.stringify(next));
  };
  const setAllSources = (on: boolean) => {
    const next = on ? srcs.map((r) => r.name) : [];
    setEnabled(next);
    localStorage.setItem(WEBRS_ENABLED_KEY, JSON.stringify(next));
  };

  const openChapters = async (rule: string, item: WebrsSearchItem) => {
    setSel({ rule, item });
    setRoads([]);
    setRoadIdx(0);
    setSubBusy("拉取选集…");
    try {
      const r = await invoke<{ roads: WebrsRoad[] }>("webrs_chapters", {
        ruleName: rule,
        url: item.url,
      });
      setRoads(r.roads ?? []);
    } catch (e) {
      setErr(String(e));
      setSel(null);
    } finally {
      setSubBusy("");
    }
  };

  const playEp = async (ep: WebrsSearchItem) => {
    if (!sel) return;
    setSubBusy("解析视频直链…（页面藏得深时会拉起嗅探，最多等 40s）");
    setErr("");
    try {
      const r = await invoke<{ url: string; referer: string; ua: string; via: string }>(
        "webrs_resolve",
        { ruleName: sel.rule, url: ep.url },
      );
      setSubBusy("正在拉起独立播放器…");
      await invoke("webrs_play", { url: r.url, referer: r.referer, ua: r.ua });
      setSubBusy(`已在独立播放器窗口播放 · ${sel.item.name} · ${ep.name}（${r.via === "webview" ? "嗅探直链" : "页面直解"}）`);
    } catch (e) {
      setErr(String(e));
      setSubBusy("");
    }
  };

  const addRule = async () => {
    setAddErr("");
    try {
      const name = await invoke<string>("webrs_add_rule", { jsonText: addText });
      setAddOpen(false);
      setAddText("");
      await refreshRules();
      setErr("");
      setSrcHint(`已添加源「${name}」，记得在列表里勾选它`);
    } catch (e) {
      setAddErr(String(e));
    }
  };

  // [lc-1503] 一键测试：全部源（不管勾没勾）各跑一次搜索，结果挂到每行徽标。
  // 测试词用搜索框关键词（空则用通用词「少女」——各站收录差异最小的探针）。
  const testAll = async () => {
    if (testing) return;
    const key = kw.trim() || "少女";
    setTesting(true);
    setAddErr("");
    const loading: Record<string, { status: string; count: number; error: string }> = {};
    for (const r of srcs) loading[r.name] = { status: "loading", count: 0, error: "" };
    setTests(loading);
    try {
      const r = await invoke<WebrsGroup[]>("webrs_search_multi", {
        keyword: key,
        rules: srcs.map((s) => s.name),
      });
      const next: Record<string, { status: string; count: number; error: string }> = {};
      for (const g of r) next[g.name] = { status: g.status, count: g.items.length, error: g.error };
      setTests(next);
    } catch (e) {
      setAddErr(String(e));
      setTests({});
    } finally {
      setTesting(false);
    }
  };

  const removeRule = async (name: string) => {
    if (!window.confirm(`删除源「${name}」？（内置源会在下次启动时恢复）`)) return;
    try {
      await invoke("webrs_remove_rule", { ruleName: name });
      await refreshRules();
      const next = (enabled ?? srcs.map((r) => r.name)).filter((x) => x !== name);
      setEnabled(next);
      localStorage.setItem(WEBRS_ENABLED_KEY, JSON.stringify(next));
      setGroups((g) => (g ? g.filter((x) => x.name !== name) : g));
    } catch (e) {
      setErr(String(e));
    }
  };

  const badge = (g: WebrsGroup) =>
    g.status === "ok" ? `${g.items.length} 个结果` : g.status === "empty" ? "无结果" : "检索失败";

  return (
    <div className="webrs">
      <div className="webrs-bar">
        <input
          value={kw}
          onChange={(e) => setKw(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && !busy && doSearch(kw)}
          placeholder="搜索资源关键词"
        />
        <button className="webrs-go" disabled={busy} onClick={() => doSearch(kw)}>
          {busy ? "搜索中" : "搜索"}
        </button>
        <button
          className="webrs-gear"
          title={`源管理（已勾选 ${enabledNames.length}/${srcs.length}）`}
          onClick={() => {
            setSettingsOpen(true);
            setAddErr("");
          }}
        >
          ⚙
        </button>
      </div>
      {busy && <div className="webrs-note">正在搜索 {enabledNames.length} 个已勾选源…</div>}
      {err && <div className="webrs-err">{err}</div>}
      {srcHint && !err && <div className="webrs-hint">{srcHint}</div>}
      {subBusy && <div className="webrs-note">{subBusy}</div>}

      {/* 二级视图：选集（返回时播退出动画；roads 在动画期间保留，内容不闪空） */}
      {(sel || subMounted) && (
        <div className={`webrs-sub${subClosing ? " view-out" : " view-in"}`}>
          <div className="webrs-crumbs">
            <span className="webrs-cur">{sel?.item.name ?? ""}</span>
            <span className="webrs-src">来源：{sel?.rule ?? ""}</span>
            <button
              className="webrs-back"
              onClick={() => setSel(null)}
              title="返回搜索结果"
            >
              ‹ 返回结果
            </button>
          </div>
          {roads.length > 1 && (
            <div className="webrs-roads">
              {roads.map((rd, i) => (
                <button key={`${rd.name}-${i}`} className={i === roadIdx ? "on" : ""} onClick={() => setRoadIdx(i)}>
                  {rd.name}
                </button>
              ))}
            </div>
          )}
          <div className="webrs-eps">
            {(roads[roadIdx]?.episodes ?? []).map((ep) => (
              <button key={ep.url} disabled={subBusy.startsWith("解析")} onClick={() => playEp(ep)} title={ep.url}>
                {ep.name}
              </button>
            ))}
          </div>
        </div>
      )}

      {/* 一级视图：按源分组的结果 */}
      {!sel && groups !== null && (
        <div className="webrs-groups">
          {groups.map((g) => (
            <div key={g.name} className={`webrs-g${g.status === "ok" ? " ok" : ""}`}>
              <button
                className="webrs-g-hd"
                onClick={() => setExpanded((m) => ({ ...m, [g.name]: !m[g.name] }))}
                title={g.error || g.name}
              >
                <span className="webrs-g-name">{g.name}</span>
                <span className={`webrs-g-badge ${g.status}`}>{badge(g)}</span>
                <span className="webrs-g-arrow">{expanded[g.name] ? "⌃" : "⌄"}</span>
              </button>
              {expanded[g.name] && g.status === "ok" && (
                <div className="webrs-g-items">
                  {g.items.map((it) => (
                    <button key={it.url} onClick={() => openChapters(g.name, it)} title={it.url}>
                      {it.name}
                    </button>
                  ))}
                </div>
              )}
            </div>
          ))}
          {groups.every((g) => g.status !== "ok") && (
            <div className="webrs-note">勾选的源都没有结果：换个关键词、换个源，或 ⚙ 里添加规则</div>
          )}
        </div>
      )}

      {/* ⚙ 源管理设置面板。portal 挂 body：弹窗留在 .content 里会被 sticky 的
          .dseg（z30 + backdrop-filter）合成穿透 —— 玻璃层画到遮罩上面成"白条"
          （[lc-1504] 用户截图实锤）。portal 后与 .dseg 同在顶层上下文，
          mask z40 > dseg z30 正常压制；顶栏 z50 仍在其上，窗口按钮不受影响。 */}
      {settingsOpen &&
        createPortal(
          <div className="ddet-mask" onClick={() => setSettingsOpen(false)}>
          <div className="sum-modal webrs-add-modal" onClick={(e) => e.stopPropagation()}>
            <div className="sum-modal-h">
              <h3>
                源管理 <span className="webrs-mgr-count">已勾选 {enabledNames.length} / {srcs.length}</span>
              </h3>
              <button className="ddet-close sum-modal-x" onClick={() => setSettingsOpen(false)}>
                ×
              </button>
            </div>
            <div className="webrs-mgr-body">
              <div className="webrs-mgr-acts">
                <button onClick={testAll} disabled={testing} title="对全部源各发一次真实搜索，检验源是否可用">
                  {testing ? "测试中…" : "⚡ 一键测试"}
                </button>
                <button onClick={() => setAllSources(true)}>全选</button>
                <button onClick={() => setAllSources(false)}>清空</button>
                <button className="webrs-mgr-addbtn" onClick={() => setAddOpen((v) => !v)}>
                  {addOpen ? "收起" : "＋ 添加源"}
                </button>
              </div>
            {addOpen && (
              <>
                <textarea
                  className="webrs-add-ta"
                  value={addText}
                  onChange={(e) => setAddText(e.target.value)}
                  placeholder="粘贴 Kazumi 规则 JSON（可从 KazumiRules 仓库复制规则文件全文），须包含 name 字段"
                  rows={10}
                />
                {addErr && <div className="webrs-err">{addErr}</div>}
                <div className="webrs-add-acts">
                  <button className="webrs-go" disabled={!addText.trim()} onClick={addRule}>
                    导入
                  </button>
                </div>
              </>
            )}
            <div className="webrs-mgr-list">
              {srcs.map((r) => {
                const t = tests[r.name];
                return (
                  <div key={r.name} className={`webrs-mgr-row${isEnabled(r.name) ? " on" : ""}`}>
                    <label className="webrs-mgr-main">
                      <input type="checkbox" checked={isEnabled(r.name)} onChange={() => toggleSource(r.name)} />
                      <span className="webrs-mgr-name">{r.name}</span>
                      {r.mode === "api" && <span className="webrs-mgr-mode">API</span>}
                    </label>
                    {t && (
                      <span
                        className={`webrs-mgr-test ${t.status}`}
                        title={t.error || (t.status === "ok" ? `搜索正常，${t.count} 条结果` : "")}
                      >
                        {t.status === "loading"
                          ? "…"
                          : t.status === "ok"
                            ? `✓ ${t.count} 条`
                            : t.status === "empty"
                              ? "无结果"
                              : "✗ 失败"}
                      </span>
                    )}
                    <button className="webrs-mgr-x" title={`删除源 ${r.name}`} onClick={() => removeRule(r.name)}>
                      ×
                    </button>
                  </div>
                );
              })}
              {srcs.length === 0 && <div className="webrs-note">没有可用源 —— 「＋ 添加源」导入一条规则</div>}
              </div>
              <div className="webrs-mgr-tip">
                勾选后关闭本面板，搜索只查已勾选的源；内置源删除后下次启动会恢复。一键测试用搜索框关键词（空则「少女」）。
              </div>
            </div>
          </div>
        </div>,
          document.body,
        )}
    </div>
  );
}

/**
 * [lc-1513] 延迟卸载：active 变 false 时先保留渲染 closing 时长（给退出动画留时间），
 * 到点再真正卸载。用于「返回上一级」的转场 —— 否则路由一切换旧视图瞬间消失。
 */
