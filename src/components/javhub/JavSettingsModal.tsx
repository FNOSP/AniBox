/* [lc-1907] JavHub 设置弹层 —— 信息结构对照 JAVCloudHub 设置页（page-settings.js SECTIONS），
   按 AniBox 架构取舍后搬运：
   · 扫描规则 / 定时扫描 / 网络与代理 / 刮削站点 / 刮削行为 / 内置 AI / 高级 → 全量落地；
   · 数据源 / 刮削目录 → 管理在「网盘接入」页，这里只读展示；
   · 存储位置 / 播放与导出 / 服务与窗口 / 桌面与托盘 → 不适用（AniBox 自身架构已覆盖）；
   · 删除 → AniBox 还没有网盘文件删除原语，暂缺。
   字段与提示文案以 JAVCloudHub 1.9.31 为蓝本改写。 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type ScanSet = { maxDepth: number; mergeMultipart: boolean; pruneMissing: boolean; videoExts: string[]; skipDirs: string[]; skipPrefixes: string[] };
type AutoSet = { enabled: boolean; intervalMinutes: number; scrapeNew: boolean; cleanupMissing: boolean; lastRunMs: number };
type NetSet = { proxy: string; trustEnv: boolean; timeoutSeconds: number; userAgent: string };
type SiteCfg = { id: string; enabled: boolean; base: string };
type ScrapeSet = { sites: SiteCfg[]; intervalSeconds: number; categories: string[]; maxAttempts: number };
type AiSet = { enabled: boolean; baseUrl: string; apiKey: string; model: string; targetLang: string };
type JavSettings = {
  scan: ScanSet;
  autoScan: AutoSet;
  network: NetSet;
  scrape: ScrapeSet;
  ai: AiSet;
  paths?: { dataDir: string; settingsFile: string; libraryFile: string; indexFile: string };
  siteDefs?: { id: string; name: string; categories: string[] }[];
};

const SECTIONS = [
  { key: "sources", label: "数据源" },
  { key: "roots", label: "刮削目录" },
  { key: "scan", label: "扫描规则" },
  { key: "auto", label: "定时扫描" },
  { key: "network", label: "网络与代理" },
  { key: "sites", label: "刮削站点" },
  { key: "behavior", label: "刮削行为" },
  { key: "ai", label: "内置 AI" },
  { key: "advanced", label: "高级" },
] as const;

const fmtTime = (ms: number) => (ms ? new Date(ms).toLocaleString("zh-CN", { hour12: false }) : "还没跑过");

export function JavSettingsModal({ onClose, onSaved }: { onClose: () => void; onSaved?: () => void }) {
  const [s, setS] = useState<JavSettings | null>(null);
  const [dirty, setDirty] = useState(false);
  const [sec, setSec] = useState<(typeof SECTIONS)[number]["key"]>("scan");
  const [saving, setSaving] = useState(false);
  const [note, setNote] = useState("");
  const [autoBusy, setAutoBusy] = useState(false);

  useEffect(() => {
    invoke<JavSettings>("jav_settings_get")
      .then((r) => setS(r))
      .catch((e) => setNote(String(e)));
  }, []);
  useEffect(() => {
    const h = (ev: KeyboardEvent) => ev.key === "Escape" && onClose();
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [onClose]);

  const patch = useCallback((fn: (d: JavSettings) => void) => {
    setS((prev) => {
      if (!prev) return prev;
      const d = JSON.parse(JSON.stringify(prev)) as JavSettings;
      fn(d);
      setDirty(true);
      return d;
    });
  }, []);

  const save = async () => {
    if (!s || saving) return;
    setSaving(true);
    setNote("");
    try {
      await invoke("jav_settings_set", { settings: s });
      setDirty(false);
      setNote("已保存");
      onSaved?.();
      window.setTimeout(() => setNote(""), 2000);
    } catch (e) {
      setNote(String(e));
    } finally {
      setSaving(false);
    }
  };

  const runAutoNow = async () => {
    if (autoBusy) return;
    setAutoBusy(true);
    setNote("正在跑一轮（重扫全部配过目录的盘）…");
    try {
      const r = await invoke<{ ranAtMs: number; results: { provider: string; scan?: { count: number; jav: number }; error?: string }[] }>("jav_auto_scan_run");
      const parts = r.results.map((x) => (x.error ? `${x.provider} 失败` : `${x.provider} ${x.scan?.count ?? 0} 项/JAV ${x.scan?.jav ?? 0}`));
      setNote(`跑完了：${parts.join("，")}`);
      setS((prev) => (prev ? { ...prev, autoScan: { ...prev.autoScan, lastRunMs: r.ranAtMs } } : prev));
    } catch (e) {
      setNote(String(e));
    } finally {
      setAutoBusy(false);
    }
  };

  const listStr = (a: string[] | undefined) => (a || []).join(", ");
  const parseList = (t: string) =>
    t
      .split(/[,，\n]/)
      .map((x) => x.trim())
      .filter(Boolean);

  const Bool = ({ label, hint, value, onChange }: { label: string; hint?: string; value: boolean; onChange: (v: boolean) => void }) => (
    <div className={`javset-field javset-bool${value ? " on" : ""}`} onClick={() => onChange(!value)} role="switch" aria-checked={value}>
      <span className="sw" />
      <span>
        <span className="bl">{label}</span>
        {hint && <div className="hint">{hint}</div>}
      </span>
    </div>
  );

  const body = useMemo(() => {
    if (!s) return null;
    switch (sec) {
      case "sources":
        return (
          <>
            <p className="javset-ro">
              数据源（网盘账号与 WebDAV）统一在「网盘接入」页管理 —— 这里不重复做登录与凭据。
              已接入的盘扫描时会自动做番号识别，命中的文件归到 JavHub。
            </p>
          </>
        );
      case "roots":
        return (
          <>
            <p className="javset-ro">
              刮削目录 = 每个盘的「媒体根目录」，在「网盘接入」页进入目录后点「加入媒体库」添加；
              支持多个根、断点续扫。这里不做目录管理，避免两处改同一份配置。
            </p>
          </>
        );
      case "scan":
        return (
          <>
            <div className="javset-field">
              <label>最大目录深度</label>
              <input type="number" min={1} max={6} value={s.scan.maxDepth}
                onChange={(e) => patch((d) => (d.scan.maxDepth = Number(e.target.value) || 3))} />
              <div className="hint">从扫描根目录往下最多翻几层。网盘上每翻一层都是一次目录枚举请求，别设太大。</div>
            </div>
            <Bool label="合并分片文件" value={s.scan.mergeMultipart}
              onChange={(v) => patch((d) => (d.scan.mergeMultipart = v))}
              hint="同一番号的 -1 / -2 / _part1 之类的分片会并成一个条目，播放时按段选。" />
            <Bool label="扫描时清理失效记录" value={s.scan.pruneMissing}
              onChange={(v) => patch((d) => (d.scan.pruneMissing = v))}
              hint="整库重扫后，网盘上已经不在的文件不再保留条目（不碰网盘上的任何文件）。关掉则只增不删。" />
            <div className="javset-field">
              <label>视频扩展名（逗号分隔）</label>
              <input type="text" value={listStr(s.scan.videoExts)}
                onChange={(e) => patch((d) => (d.scan.videoExts = parseList(e.target.value)))} />
              <div className="hint">改错了会把非视频文件也收进库。</div>
            </div>
            <div className="javset-field">
              <label>跳过的目录名（逗号分隔，大小写不敏感）</label>
              <input type="text" value={listStr(s.scan.skipDirs)}
                onChange={(e) => patch((d) => (d.scan.skipDirs = parseList(e.target.value)))} />
              <div className="hint">系统回收站、@eaDir 缩略图缓存之类的放在这里。</div>
            </div>
            <div className="javset-field">
              <label>跳过的名称前缀（逗号分隔）</label>
              <input type="text" value={listStr(s.scan.skipPrefixes)}
                onChange={(e) => patch((d) => (d.scan.skipPrefixes = parseList(e.target.value)))} />
              <div className="hint">默认只有 "."（隐藏项）。慎加 "@" —— 网盘资源里「@某某合集」很常见，按前缀跳会整片漏扫。</div>
            </div>
          </>
        );
      case "auto":
        return (
          <>
            <Bool label="启用定时自动扫描" value={s.autoScan.enabled}
              onChange={(v) => patch((d) => (d.autoScan.enabled = v))}
              hint="打开后每隔一段时间自动跑一轮：重扫目录 → 清理失效 → 把待刮削队列跑一遍。默认关闭，关着时后台不会有任何动作。" />
            <div className="javset-field">
              <label>间隔（分钟，5 ~ 10080）</label>
              <input type="number" min={5} max={10080} value={s.autoScan.intervalMinutes}
                onChange={(e) => patch((d) => (d.autoScan.intervalMinutes = Number(e.target.value) || 360))} />
              <div className="hint">60 = 每小时，360 = 每 6 小时，1440 = 每天。网盘目录枚举是实打实的 API 请求，别设太密。</div>
            </div>
            <Bool label="自动刮削新入库的条目" value={s.autoScan.scrapeNew}
              onChange={(v) => patch((d) => (d.autoScan.scrapeNew = v))}
              hint="扫完顺手把「待刮削」队列跑一遍。只刮新的，不重试历史失败条目。" />
            <Bool label="清理文件已消失的条目" value={s.autoScan.cleanupMissing}
              onChange={(v) => patch((d) => (d.autoScan.cleanupMissing = v))}
              hint="配合「扫描时清理失效记录」使用；网盘掉线导致的误清，重扫一次就会回来。" />
            <div className="javset-field">
              <label>上次运行</label>
              <div className="javset-ro">{fmtTime(s.autoScan.lastRunMs)}</div>
            </div>
            <button className="javset-btn ghost" disabled={autoBusy} onClick={() => void runAutoNow()}>
              {autoBusy ? "正在跑…" : "立即跑一轮"}
            </button>
          </>
        );
      case "network":
        return (
          <>
            <Bool label="读取系统代理" value={s.network.trustEnv}
              onChange={(v) => patch((d) => (d.network.trustEnv = v))}
              hint="默认开。环境变量（HTTP_PROXY 等）与 Windows「系统代理」注册表都会读 —— Clash / v2rayN 开系统代理时不给进程塞环境变量，这层必须读注册表才能看见。" />
            <div className="javset-field">
              <label>代理地址（留空 = 用上面的系统代理）</label>
              <input type="text" placeholder="http://127.0.0.1:10808 或 socks5://…" value={s.network.proxy}
                onChange={(e) => patch((d) => (d.network.proxy = e.target.value))} />
              <div className="hint">填了就以这里为准（手写优先）。两个都空 = 直连，只有镜像轨能用。</div>
            </div>
            <div className="javset-field">
              <label>请求超时（秒）</label>
              <input type="number" min={5} max={120} value={s.network.timeoutSeconds}
                onChange={(e) => patch((d) => (d.network.timeoutSeconds = Number(e.target.value) || 25))} />
            </div>
            <div className="javset-field">
              <label>User-Agent</label>
              <textarea value={s.network.userAgent} onChange={(e) => patch((d) => (d.network.userAgent = e.target.value))} />
              <div className="hint">所有刮削请求都会带这个 UA。某个站对你的 UA 特别敏感时，换成浏览器里那串。</div>
            </div>
          </>
        );
      case "sites": {
        const defById = new Map((s.siteDefs || []).map((d) => [d.id, d]));
        const move = (i: number, dir: -1 | 1) => {
          const j = i + dir;
          if (j < 0 || j >= s.scrape.sites.length) return;
          patch((d) => {
            const [row] = d.scrape.sites.splice(i, 1);
            d.scrape.sites.splice(j, 0, row);
          });
        };
        return (
          <>
            <p className="javset-ro">勾上要用的站点，用 ↑ ↓ 调顺序 —— 越靠上越先尝试，前一个失败才轮到下一个。地址留空 = 内置默认（r18.dev / FC2 / jav321 支持填镜像域）。</p>
            {s.scrape.sites.map((site, i) => {
              const def = defById.get(site.id);
              return (
                <div key={site.id} className={`javset-site${site.enabled ? "" : " off"}`}>
                  <div className={`javset-field javset-bool${site.enabled ? " on" : ""}`} style={{ margin: 0 }}
                    onClick={() => patch((d) => (d.scrape.sites[i].enabled = !d.scrape.sites[i].enabled))}>
                    <span className="sw" />
                  </div>
                  <span className="sr-name" title={def?.name}>{def?.name || site.id}</span>
                  <input placeholder="自定义地址（留空 = 默认）" value={site.base}
                    onChange={(e) => patch((d) => (d.scrape.sites[i].base = e.target.value))} />
                  <button className="mv" title="上移" onClick={() => move(i, -1)}>↑</button>
                  <button className="mv" title="下移" onClick={() => move(i, 1)}>↓</button>
                </div>
              );
            })}
            <p className="javset-ro" style={{ opacity: 0.75 }}>
              JavBus / JavDB / metatube / 自定义 CSS 站点引擎待接入，接入后会出现在这份列表里。
            </p>
          </>
        );
      }
      case "behavior":
        return (
          <>
            <div className="javset-field">
              <label>请求间隔（秒）</label>
              <input type="number" min={0.2} max={60} step={0.1} value={s.scrape.intervalSeconds}
                onChange={(e) => patch((d) => (d.scrape.intervalSeconds = Number(e.target.value) || 1.5))} />
              <div className="hint">每个请求之间至少等这么久。对站点友好一点，自己也不容易被封。</div>
            </div>
            <div className="javset-field">
              <label>参与刮削的分类（fc2 / censored / uncensored）</label>
              <input type="text" value={listStr(s.scrape.categories)}
                onChange={(e) => patch((d) => (d.scrape.categories = parseList(e.target.value)))} />
              <div className="hint">无番号的 unknown 不在其中 —— 它本来就没法按番号查。</div>
            </div>
            <p className="javset-ro">并发数暂为串行（1）—— 刮削走镜像轨 + 礼貌间隔，串行最稳；后续接入 HTML 站点引擎后再放开。</p>
          </>
        );
      case "ai":
        return (
          <>
            <Bool label="启用内置 AI（标题汉化）" value={s.ai.enabled}
              onChange={(v) => patch((d) => (d.ai.enabled = v))}
              hint="刮削完成后把外文标题批量译成目标语言，卡片优先显示译名。兼容 OpenAI Chat Completions 协议（DeepSeek / 中转 / Ollama 均可）。" />
            <div className="javset-field">
              <label>接口地址（Base URL，不含 /chat/completions）</label>
              <input type="text" value={s.ai.baseUrl}
                onChange={(e) => patch((d) => (d.ai.baseUrl = e.target.value))} />
            </div>
            <div className="javset-field">
              <label>API Key</label>
              <input type="password" value={s.ai.apiKey}
                onChange={(e) => patch((d) => (d.ai.apiKey = e.target.value))} />
              <div className="hint">只存本机 jav-settings.json，不上传、不入库。</div>
            </div>
            <div className="javset-field">
              <label>模型</label>
              <input type="text" value={s.ai.model} onChange={(e) => patch((d) => (d.ai.model = e.target.value))} />
            </div>
            <div className="javset-field">
              <label>目标语言</label>
              <input type="text" placeholder="zh / zh-TW / en / ja" value={s.ai.targetLang}
                onChange={(e) => patch((d) => (d.ai.targetLang = e.target.value))} />
            </div>
          </>
        );
      case "advanced":
        return (
          <>
            <div className="javset-field">
              <label>数据目录</label>
              <div className="javset-ro">{s.paths?.dataDir || "（未知）"}</div>
              <div className="hint">jav-settings.json（本面板）、jav-library.json（番号条目）、cloud-library.json（网盘索引）都在这里。</div>
            </div>
            <p className="javset-ro">
              JavBus / JavDB / metatube / 自定义站点引擎、STRM 导出、网盘删除暂未接入 ——
              接入后对应设置节会出现在这里。
            </p>
          </>
        );
    }
  }, [s, sec, autoBusy]);

  return (
    <div className="javset-scrim" onClick={onClose}>
      <div className="javset" onClick={(e) => e.stopPropagation()}>
        <div className="javset-nav">
          <div className="javset-navhead">JavHub 设置</div>
          {SECTIONS.map((x) => (
            <button key={x.key} type="button" className={`javset-navitem${sec === x.key ? " on" : ""}`} onClick={() => setSec(x.key)}>
              {x.label}
            </button>
          ))}
        </div>
        <div className="javset-body">
          <div className="javset-sechead">
            <h4>{SECTIONS.find((x) => x.key === sec)?.label}</h4>
            {sec === "sources" && <p>网盘账号在「网盘接入」页管理</p>}
            {sec === "roots" && <p>媒体根目录在「网盘接入」页添加</p>}
          </div>
          <div className="javset-fields">{body || <p className="javset-ro">加载中…</p>}</div>
          <div className="javset-foot">
            <span className="dirty">{note || (dirty ? "有未保存的改动" : "")}</span>
            <button className="javset-btn ghost" onClick={onClose}>关闭</button>
            <button className="javset-btn" disabled={!s || !dirty || saving} onClick={() => void save()}>
              {saving ? "保存中…" : "保存"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
