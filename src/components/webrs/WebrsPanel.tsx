/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createPortal } from "react-dom";
import { useDelayedUnmount } from "../../hooks/useDelayedUnmount";

export type WebrsSearchItem = { name: string; url: string };

export type WebrsRoad = { name: string; index: number; episodes: WebrsSearchItem[] };

// [lc-1606] 新增 captcha 状态：该源返回了反爬验证挑战页，需要人机验证
export type WebrsGroup = { name: string; status: "ok" | "empty" | "error" | "captcha"; error: string; items: WebrsSearchItem[] };

export type WebrsRuleInfo = { name: string; version?: string; mode?: string };

// [lc-1608] 在线规则目录条目（对应 Rust CatalogItem）+ 可更新项
export type WebrsCatalogItem = {
  name: string;
  version: string;
  author?: string;
  useNativePlayer?: boolean;
  lastUpdate?: number;
};
export type WebrsUpdateItem = { name: string; localVersion: string; remoteVersion: string };


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
  // [lc-1608] 在线规则目录：目录列表 + 可更新项 + 检查进度 + 行内安装/更新忙态
  const [catalog, setCatalog] = useState<WebrsCatalogItem[] | null>(null);
  const [updates, setUpdates] = useState<WebrsUpdateItem[]>([]);
  const [catBusy, setCatBusy] = useState(false);
  const [catMsg, setCatMsg] = useState<{ kind: "ok" | "err" | "note"; text: string } | null>(null);
  const [rowBusy, setRowBusy] = useState<Record<string, "install" | "update">>({});
  // 二级视图：选集
  const [sel, setSel] = useState<{ rule: string; item: WebrsSearchItem } | null>(null);
  const [roads, setRoads] = useState<WebrsRoad[]>([]);
  const [roadIdx, setRoadIdx] = useState(0);
  const [subBusy, setSubBusy] = useState("");

  // [lc-1606] 反爬验证：type1 的验证码图 + 输入；captchaMsg 是整体状态反馈
  const [captcha, setCaptcha] = useState<{ rule: string; image: string } | null>(null);
  const [captchaMsg, setCaptchaMsg] = useState<{ rule: string; status: "loading" | "verified" | "failed"; error: string } | null>(null);
  const [code, setCode] = useState("");
  // 同一源避免自动重复发起（验证成功后重搜若仍挑战 → 最多再试 2 次，防死循环）
  const captchaAttempts = useRef<Record<string, number>>({});
  const captchaBusyRef = useRef(false);

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

  // [lc-1606] 发起反爬验证（Rust 建隐藏窗加载搜索页，按 captchaType 自动/交互完成）
  const startCaptcha = useCallback(async (rule: string, keyword: string) => {
    captchaBusyRef.current = true;
    setCaptcha(null);
    setCode("");
    setCaptchaMsg({ rule, status: "loading", error: "" });
    try {
      await invoke("webrs_captcha_start", { ruleName: rule, keyword });
    } catch (e) {
      captchaBusyRef.current = false;
      setCaptchaMsg({ rule, status: "failed", error: String(e) });
    }
  }, []);

  const submitCaptcha = useCallback(async () => {
    if (!captcha) return;
    const c = code.trim();
    if (!c) return;
    try {
      await invoke("webrs_captcha_submit", { ruleName: captcha.rule, code: c });
      setCode("");
      setCaptcha(null);
      setCaptchaMsg({ rule: captcha.rule, status: "loading", error: "" });
    } catch (e) {
      setCaptchaMsg({ rule: captcha.rule, status: "failed", error: String(e) });
    }
  }, [captcha, code]);

  const cancelCaptcha = useCallback(async () => {
    captchaBusyRef.current = false;
    setCaptcha(null);
    setCode("");
    setCaptchaMsg(null);
    try {
      await invoke("webrs_captcha_cancel");
    } catch {
      /* 忽略：窗口可能已自行销毁 */
    }
  }, []);

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
        // [lc-1606] 有源需要人机验证 → 自动发起（一次一个，避免多隐藏窗）
        const need = r.filter((g) => g.status === "captcha");
        if (need.length > 0 && !captchaBusyRef.current) {
          const target = need.find((g) => (captchaAttempts.current[g.name] ?? 0) < 2) ?? need[0];
          const n = captchaAttempts.current[target.name] ?? 0;
          if (n < 2) {
            captchaAttempts.current[target.name] = n + 1;
            await startCaptcha(target.name, key);
          } else {
            setSrcHint(`源「${target.name}」需要人机验证（已尝试多次），可点该行「去验证」重试`);
          }
        }
      } catch (e) {
        setErr(String(e));
        setGroups([]);
      } finally {
        setBusy(false);
      }
    },
    [srcs, isEnabled, startCaptcha],
  );

  // [lc-1606] 验证事件：type1 收到图 → 弹窗；status verified → 自动重搜
  const doSearchRef = useRef(doSearch);
  doSearchRef.current = doSearch;
  const kwRef = useRef(kw);
  kwRef.current = kw;
  useEffect(() => {
    let alive = true;
    let un1: (() => void) | undefined;
    let un2: (() => void) | undefined;
    listen<{ rule: string; image: string }>("webrs-captcha", (e) => {
      if (!alive) return;
      setCaptcha({ rule: e.payload.rule, image: e.payload.image });
    }).then((u) => {
      if (alive) un1 = u;
      else u();
    });
    listen<{ rule: string; status: string; error: string }>("webrs-captcha-status", (e) => {
      if (!alive) return;
      const p = e.payload;
      if (p.status === "verified") {
        captchaBusyRef.current = false;
        setCaptcha(null);
        setCaptchaMsg({ rule: p.rule, status: "verified", error: "" });
        // 自动重试：cookie 已入 jar，重搜这次就能拿到结果
        doSearchRef.current(kwRef.current);
      } else if (p.status === "failed") {
        captchaBusyRef.current = false;
        setCaptcha(null);
        setCaptchaMsg({ rule: p.rule, status: "failed", error: p.error });
      }
    }).then((u) => {
      if (alive) un2 = u;
      else u();
    });
    return () => {
      alive = false;
      un1?.();
      un2?.();
    };
  }, []);

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

  // ───────────── [lc-1608] 在线规则目录：检查 / 安装 / 更新 / 全部更新 ─────────────
  // 本地版本表（小写名 → 版本），用于给目录行打「未装 / 已装 vX / 可更新」状态。
  const localVer = useMemo(() => {
    const m: Record<string, string> = {};
    for (const r of srcs) if (r.version) m[r.name.toLowerCase()] = r.version;
    return m;
  }, [srcs]);
  const updateMap = useMemo(() => {
    const m: Record<string, WebrsUpdateItem> = {};
    for (const u of updates) m[u.name.toLowerCase()] = u;
    return m;
  }, [updates]);

  const checkUpdates = useCallback(async (force: boolean) => {
    if (catBusy) return;
    setCatBusy(true);
    setCatMsg({ kind: "note", text: force ? "正在刷新在线规则目录…" : "正在读取在线规则目录…" });
    try {
      const c = await invoke<{ items: WebrsCatalogItem[] }>("webrs_catalog", { force });
      setCatalog(c.items ?? []);
      // 目录已（内存）缓存，第二次调用很快；可更新判据在 Rust（Kazumi _remoteIsNewer 口径）
      const u = await invoke<{ items: WebrsUpdateItem[] }>("webrs_check_updates");
      setUpdates(u.items ?? []);
      setCatMsg({
        kind: "ok",
        text: `目录 ${c.items?.length ?? 0} 条规则，${u.items?.length ?? 0} 条可更新`,
      });
    } catch (e) {
      setCatMsg({ kind: "err", text: String(e) });
    } finally {
      setCatBusy(false);
    }
  }, [catBusy]);

  // 安装 / 更新单条。串行由调用方保证（行内按钮天然一次一个）。
  const installOrUpdate = useCallback(
    async (name: string, kind: "install" | "update") => {
      setRowBusy((m) => ({ ...m, [name]: kind }));
      setCatMsg({ kind: "note", text: `${kind === "install" ? "安装" : "更新"}「${name}」…` });
      try {
        const r = await invoke<{ name: string; version: string }>(
          kind === "install" ? "webrs_install_rule" : "webrs_update_rule",
          { name },
        );
        await refreshRules();
        const u = await invoke<{ items: WebrsUpdateItem[] }>("webrs_check_updates");
        setUpdates(u.items ?? []);
        setCatMsg({ kind: "ok", text: `已${kind === "install" ? "安装" : "更新"}「${r.name}」v${r.version}` });
      } catch (e) {
        setCatMsg({ kind: "err", text: `${kind === "install" ? "安装" : "更新"}「${name}」失败：${e}` });
      } finally {
        setRowBusy((m) => {
          const n = { ...m };
          delete n[name];
          return n;
        });
      }
    },
    [refreshRules],
  );

  // 全部更新：**串行**逐个调用（不并发轰炸上游），带 i/n 进度。
  const updateAll = useCallback(async () => {
    if (catBusy) return;
    const targets = updates.map((u) => u.name);
    if (targets.length === 0) {
      setCatMsg({ kind: "note", text: "没有可更新的规则" });
      return;
    }
    setCatBusy(true);
    let ok = 0;
    let fail = 0;
    for (let i = 0; i < targets.length; i++) {
      const name = targets[i];
      setRowBusy((m) => ({ ...m, [name]: "update" }));
      setCatMsg({ kind: "note", text: `更新中 ${i + 1}/${targets.length}：「${name}」…` });
      try {
        await invoke("webrs_update_rule", { name });
        ok++;
      } catch {
        fail++;
      } finally {
        setRowBusy((m) => {
          const n = { ...m };
          delete n[name];
          return n;
        });
      }
    }
    await refreshRules();
    try {
      const u = await invoke<{ items: WebrsUpdateItem[] }>("webrs_check_updates");
      setUpdates(u.items ?? []);
    } catch {
      /* 忽略：至少把已更新的结果报出去 */
    }
    setCatBusy(false);
    setCatMsg({ kind: fail ? "err" : "ok", text: `全部更新完成：成功 ${ok} 条${fail ? `，失败 ${fail} 条` : ""}` });
  }, [catBusy, updates, refreshRules]);

  const badge = (g: WebrsGroup) =>
    g.status === "ok"
      ? `${g.items.length} 个结果`
      : g.status === "empty"
        ? "无结果"
        : g.status === "captcha"
          ? "需要验证"
          : "检索失败";

  return (
    <div className="webrs">
      {/* [lc-1609] 合规：常驻提示 —— 在线资源全部来自第三方网站，本软件不提供/不存储任何资源 */}
      <div className="webrs-hint">
        资源来自第三方网站，本软件不提供、不存储任何资源 · 详见「设置 → 关于 → 免责声明」
      </div>
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
      {/* [lc-1606] 反爬验证状态 */}
      {captchaMsg?.status === "loading" && (
        <div className="webrs-note">人机验证 · {captchaMsg.rule} —— 正在等待网站响应…</div>
      )}
      {captchaMsg?.status === "verified" && (
        <div className="webrs-captcha-ok">✓ 验证通过 · {captchaMsg.rule} —— 正在重新检索…</div>
      )}
      {captchaMsg?.status === "failed" && (
        <div className="webrs-err">人机验证失败 · {captchaMsg.rule}：{captchaMsg.error}</div>
      )}

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
              {g.status === "captcha" ? (
                // [lc-1606] 需要人机验证：整行不是折叠按钮，右侧给「去验证」入口
                <div className="webrs-g-hd webrs-g-captcha">
                  <span className="webrs-g-name">{g.name}</span>
                  <span className="webrs-g-badge captcha">{badge(g)}</span>
                  <button
                    className="webrs-g-verify"
                    onClick={() => startCaptcha(g.name, kw)}
                    title="重新发起人机验证"
                  >
                    去验证
                  </button>
                </div>
              ) : (
                <button
                  className="webrs-g-hd"
                  onClick={() => setExpanded((m) => ({ ...m, [g.name]: !m[g.name] }))}
                  title={g.error || g.name}
                >
                  <span className="webrs-g-name">{g.name}</span>
                  <span className={`webrs-g-badge ${g.status}`}>{badge(g)}</span>
                  <span className="webrs-g-arrow">{expanded[g.name] ? "⌃" : "⌄"}</span>
                </button>
              )}
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

              {/* [lc-1608] 在线规则目录（KazumiRules index.json）：检查更新 / 逐条安装·更新 / 全部更新 */}
              <div className="webrs-cat">
                <div className="webrs-cat-hd">
                  <span className="webrs-cat-title">在线规则</span>
                  <button onClick={() => checkUpdates(true)} disabled={catBusy}>
                    {catBusy ? "检查中…" : "检查更新"}
                  </button>
                  <button
                    className="webrs-cat-upall"
                    onClick={updateAll}
                    disabled={catBusy || updates.length === 0}
                    title={updates.length ? `更新 ${updates.length} 条可更新规则` : "暂无可更新规则"}
                  >
                    全部更新{updates.length ? ` (${updates.length})` : ""}
                  </button>
                </div>
                {catMsg && (
                  <div className={`webrs-cat-msg ${catMsg.kind === "err" ? "err" : catMsg.kind === "ok" ? "ok" : ""}`}>
                    {catMsg.text}
                  </div>
                )}
                {catalog && (
                  <div className="webrs-cat-list">
                    {catalog.map((it) => {
                      const lv = localVer[it.name.toLowerCase()];
                      const up = updateMap[it.name.toLowerCase()];
                      const busy = rowBusy[it.name];
                      const state = !lv ? "new" : up ? "update" : "installed";
                      return (
                        <div key={it.name} className="webrs-cat-row">
                          <span className="webrs-cat-name" title={it.author ? `${it.name} · ${it.author}` : it.name}>
                            {it.name}
                          </span>
                          <span className="webrs-cat-ver">v{it.version}</span>
                          <span className={`webrs-cat-state ${state}`}>
                            {state === "new"
                              ? "未装"
                              : state === "update"
                                ? `可更新 v${lv} → v${it.version}`
                                : `已装 v${lv}`}
                          </span>
                          {state === "new" && (
                            <button
                              className="webrs-cat-act"
                              disabled={!!busy || catBusy}
                              onClick={() => installOrUpdate(it.name, "install")}
                            >
                              {busy === "install" ? "安装中…" : "安装"}
                            </button>
                          )}
                          {state === "update" && (
                            <button
                              className="webrs-cat-act on"
                              disabled={!!busy || catBusy}
                              onClick={() => installOrUpdate(it.name, "update")}
                            >
                              {busy === "update" ? "更新中…" : "更新"}
                            </button>
                          )}
                        </div>
                      );
                    })}
                    {catalog.length === 0 && <div className="webrs-note">在线目录为空</div>}
                  </div>
                )}
                {!catalog && !catBusy && (
                  <div className="webrs-note">点「检查更新」从 KazumiRules 拉取在线规则目录（带 6 小时缓存）</div>
                )}
              </div>

              <div className="webrs-mgr-tip">
                勾选后关闭本面板，搜索只查已勾选的源；内置源删除后下次启动会恢复。一键测试用搜索框关键词（空则「少女」）。
              </div>
            </div>
          </div>
        </div>,
          document.body,
        )}

      {/* [lc-1606] type1 图片验证码弹窗：显示抓到的图 + 输入框 → webrs_captcha_submit。
          portal 挂 body（同源管理弹窗，避开 .dseg 合成穿透）。 */}
      {captcha &&
        createPortal(
          <div className="ddet-mask">
            <div className="sum-modal webrs-captcha-modal" onClick={(e) => e.stopPropagation()}>
              <div className="sum-modal-h">
                <h3>
                  人机验证 <span className="webrs-mgr-count">{captcha.rule}</span>
                </h3>
                <button className="ddet-close sum-modal-x" onClick={cancelCaptcha}>
                  ×
                </button>
              </div>
              <div className="webrs-captcha-body">
                <div className="webrs-captcha-tip">该来源需要验证，请输入下图中的字符以继续检索。</div>
                <div className="webrs-captcha-imgwrap">
                  <img className="webrs-captcha-img" src={captcha.image} alt="验证码" />
                </div>
                <input
                  className="webrs-captcha-input"
                  value={code}
                  autoFocus
                  spellCheck={false}
                  autoComplete="off"
                  placeholder="输入图片中的字符"
                  onChange={(e) => setCode(e.target.value)}
                  onKeyDown={(e) => e.key === "Enter" && submitCaptcha()}
                />
                <div className="webrs-captcha-acts">
                  <button className="webrs-captcha-cancel" onClick={cancelCaptcha}>
                    取消
                  </button>
                  <button className="webrs-go" disabled={!code.trim()} onClick={submitCaptcha}>
                    验证
                  </button>
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
