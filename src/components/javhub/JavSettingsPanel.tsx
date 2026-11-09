/* [lc-1907] JavHub 设置 —— 页面布局对照 JAVCloudHub 设置页（page-settings.js）：
   顶部搜索条 + 分节字段卡（.setitem 同款：粗标签 + 灰色 key 路径 + 控件 + hint）；
   原版左侧竖导航改为顶部横排胶囊。字段文案以 JAVCloudHub 1.9.31 为蓝本改写。
   [lc-1951] 不再以弹窗打开：作为 JavHub 页的「设置」标签页内嵌渲染（javset.embed）。
   按 AniBox 架构取舍：
   · 扫描规则 / 定时扫描 / 网络与代理 / 刮削站点 / 刮削行为 / 内置 AI / 高级 → 全量落地；
   · 数据源 → 网盘指回「云盘接入」页，飞牛媒体库逐库开关 + 扫描按钮；
   · 存储位置 / 播放与导出 / 服务与窗口 / 桌面与托盘 → 不适用（AniBox 自身架构已覆盖）；
   · 删除 → AniBox 还没有网盘文件删除原语，暂缺。 */
import { useCallback, useEffect, useState } from "react";
import {
  Bot as IcoBot,
  Clock as IcoClock,
  Database as IcoDb,
  Globe as IcoGlobe,
  Search as IcoSearch,
  Shield as IcoShield,
  Sparkles as IcoSparkle,
  Wrench as IcoWrench,
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDirDialog } from "@tauri-apps/plugin-dialog";

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
type FnosLib = { guid: string; title: string; category: string; enabled: boolean };
type JavSettingsWithLocal = JavSettings & { local?: { dirs: string[] } };

const SECTIONS = [
  { key: "sources", label: "数据源", icon: IcoDb },
  { key: "scan", label: "扫描规则", icon: IcoSearch },
  { key: "auto", label: "定时扫描", icon: IcoClock },
  { key: "network", label: "网络与代理", icon: IcoShield },
  { key: "sites", label: "刮削站点", icon: IcoGlobe },
  { key: "behavior", label: "刮削行为", icon: IcoSparkle },
  { key: "ai", label: "内置 AI", icon: IcoBot },
  { key: "advanced", label: "高级", icon: IcoWrench },
] as const;
type SecKey = (typeof SECTIONS)[number]["key"];

const fmtTime = (ms: number) => (ms ? new Date(ms).toLocaleString("zh-CN", { hour12: false }) : "还没跑过");

/** hint 里把 URL / 路径渲染成 code（照抄 linkify） */
const linkify = (t: string) =>
  t.split(/(https?:\/\/[^\s，。；]+|socks5?:\/\/[^\s，。；]+|[A-Za-z]:\\[^\s，。；]+)/g).map((seg, i) =>
    /^(https?:|socks5?:|[A-Za-z]:\\)/.test(seg) ? <code key={i}>{seg}</code> : seg,
  );

export function JavSettingsPanel({ onSaved }: { onSaved?: () => void }) {
  const [s, setS] = useState<JavSettings | null>(null);
  const [dirty, setDirty] = useState(false);
  const [sec, setSec] = useState<SecKey>("sources");
  const [q, setQ] = useState("");
  const [saving, setSaving] = useState(false);
  const [note, setNote] = useState("");
  const [autoBusy, setAutoBusy] = useState(false);
  const [fnosLibs, setFnosLibs] = useState<FnosLib[] | null>(null);
  const [fnosBusy, setFnosBusy] = useState(false);
  const [fnosNote, setFnosNote] = useState("");

  useEffect(() => {
    invoke<JavSettings>("jav_settings_get")
      .then((r) => setS(r))
      .catch((e) => setNote(String(e)));
  }, []);
  const loadFnos = useCallback(async () => {
    try {
      const r = await invoke<{ libs: FnosLib[] }>("jav_fnos_libs");
      setFnosLibs(r.libs);
    } catch {
      setFnosLibs([]);
    }
  }, []);
  useEffect(() => {
    void loadFnos();
  }, [loadFnos]);

  const addLocalDir = async () => {
    const picked = await openDirDialog({ directory: true, multiple: false, title: "选择本地视频文件夹" });
    if (typeof picked !== "string" || !picked.trim()) return;
    const dir = picked.trim();
    setS((prev) => {
      if (!prev) return prev;
      const dirs = (prev as JavSettingsWithLocal).local?.dirs ?? [];
      if (dirs.some((x) => x.toLowerCase() === dir.toLowerCase())) return prev;
      const next = { ...prev, local: { dirs: [...dirs, dir] } } as JavSettingsWithLocal;
      setDirty(true);
      return next;
    });
    setNote("本地文件夹已添加，保存后点「扫描本地文件夹」生效");
  };
  const removeLocalDir = (dir: string) => {
    setS((prev) => {
      if (!prev) return prev;
      const dirs = ((prev as JavSettingsWithLocal).local?.dirs ?? []).filter((x) => x !== dir);
      setDirty(true);
      return { ...prev, local: { dirs } } as JavSettingsWithLocal;
    });
  };
  const scanLocal = async () => {
    if (fnosBusy) return;
    setFnosBusy(true);
    setFnosNote("正在扫描本地文件夹（遍历 + 番号识别）…");
    try {
      const r = await invoke<{ results: { dir: string; videos: number; javDetected: number }[] }>("jav_local_scan");
      const parts = r.results.map((x) => `${x.dir.split(/[\/]/).pop()} ${x.javDetected}/${x.videos} 条命中番号`);
      setFnosNote(`扫完了：${parts.join("，")}（刮削在 JavHub 页点「刮削本盘」）`);
    } catch (e) {
      setFnosNote(String(e));
    } finally {
      setFnosBusy(false);
    }
  };

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
  const parseList = (t: string) => t.split(/[,，\n]/).map((x) => x.trim()).filter(Boolean);

  /* ── 字段卡（对照 .setitem）── */
  const Item = ({ label, path, hint, children }: { label?: string; path?: string; hint?: string; children: React.ReactNode }) => (
    <div className="javset-item">
      {label && (
        <label>
          {label}
          {path && <span className="key">{path}</span>}
        </label>
      )}
      {children}
      {hint && <div className="hint">{linkify(hint)}</div>}
    </div>
  );
  const Bool = ({ path, label, hint, value, onChange }: { path: string; label: string; hint?: string; value: boolean; onChange: (v: boolean) => void }) => (
    <div className={`javset-item boolrow javset-bool${value ? " on" : ""}`} onClick={() => onChange(!value)} role="switch" aria-checked={value}>
      <label>
        <span className="sw" />
        <span className="bl">{label}</span>
      </label>
      <span>
        {hint && <div className="hint">{linkify(hint)}</div>}
        <span className="key">{path}</span>
      </span>
    </div>
  );
  const Ro = ({ children, style }: { children: React.ReactNode; style?: React.CSSProperties }) => (
    <div className="javset-ro" style={style}>{children}</div>
  );

  /* ── 各节内容（字段 + 搜索关键词）── */
  const buildSections = (s2: JavSettings): Record<SecKey, { key: string; label: string; keywords: string; el: React.ReactNode }[]> => {
    const sitesPart = (
      <>
        <Ro>
          勾上要用的站点，用 ↑ ↓ 调顺序 —— <b>越靠上越先尝试</b>，前一个失败才轮到下一个。
          地址留空 = 内置默认；填了自定义地址（镜像域）就直连那个域，不走镜像轨。
          JavBus / JavDB / metatube / 自定义 CSS 站点引擎待接入，接入后会出现在这份列表里。
        </Ro>
        {s2.scrape.sites.map((site, i) => {
          const def = (s2.siteDefs || []).find((d) => d.id === site.id);
          const move = (dir: -1 | 1) => {
            const j = i + dir;
            if (j < 0 || j >= s2.scrape.sites.length) return;
            patch((d) => {
              const [row] = d.scrape.sites.splice(i, 1);
              d.scrape.sites.splice(j, 0, row);
            });
          };
          return (
            <div key={site.id} className={`javset-site${site.enabled ? "" : " off"}`}>
              <div className={`javset-item boolrow javset-bool${site.enabled ? " on" : ""}`} style={{ margin: 0 }}
                onClick={() => patch((d) => (d.scrape.sites[i].enabled = !d.scrape.sites[i].enabled))}>
                <span className="sw" />
              </div>
              <span className="sr-name" title={def?.name}>{def?.name || site.id}</span>
              <span className="cat">{(def?.categories || []).join("/")}</span>
              <input placeholder="自定义地址（留空 = 默认）" value={site.base}
                onChange={(e) => patch((d) => (d.scrape.sites[i].base = e.target.value))} />
              <button className="mv" title="上移" onClick={() => move(-1)}>↑</button>
              <button className="mv" title="下移" onClick={() => move(1)}>↓</button>
            </div>
          );
        })}
      </>
    );
    const fnosPart = (
      <>
        <Ro>
          网盘在「云盘接入」页管理；勾选要刮削的<b>飞牛媒体库</b>，元数据只写本机（不写飞牛）。
        </Ro>
        {(fnosLibs || []).map((l) => (
          <div key={l.guid} className={`javset-site${l.enabled ? "" : " off"}`}>
            <div className={`javset-item boolrow javset-bool${l.enabled ? " on" : ""}`} style={{ margin: 0 }}
              onClick={async () => {
                const next = !l.enabled;
                setFnosLibs((prev) => (prev || []).map((x) => (x.guid === l.guid ? { ...x, enabled: next } : x)));
                try {
                  await invoke("jav_fnos_set_lib", { guid: l.guid, title: l.title, enabled: next });
                } catch (e) {
                  setFnosNote(String(e));
                  void loadFnos();
                }
              }}>
              <span className="sw" />
            </div>
            <span className="sr-name" title={l.title}>{l.title}</span>
            <span className="cat">{l.category || "飞牛媒体库"}</span>
          </div>
        ))}
        {fnosLibs && fnosLibs.length === 0 && <Ro>没有读到飞牛媒体库（未登录或 NAS 上还没有建库）。</Ro>}
        <div className="jvh-src-actions">
          <button className="javset-btn ghost" disabled={fnosBusy || !(fnosLibs || []).some((x) => x.enabled)}
            onClick={async () => {
              setFnosBusy(true);
              setFnosNote("正在扫描启用的媒体库（翻页读取 + 番号识别）…");
              try {
                const r = await invoke<{ results: { title?: string; error?: string; items?: number; javDetected?: number }[] }>("jav_fnos_scan");
                const parts = r.results.map((x) => (x.error ? `${x.title ?? ""} 失败` : `${x.title} ${x.javDetected}/${x.items} 条命中番号`));
                setFnosNote(`扫完了：${parts.join("，")}`);
              } catch (e) {
                setFnosNote(String(e));
              } finally {
                setFnosBusy(false);
              }
            }}>
            {fnosBusy ? "扫描中…" : "扫描启用的媒体库"}
          </button>
        </div>
        <div className="jvh-subhead">本地文件夹</div>
        {(((s as JavSettingsWithLocal).local?.dirs ?? []) as string[]).map((dir) => (
          <div key={dir} className="javset-site">
            <span className="sr-name" title={dir}>{dir.split(/[\/]/).pop()}</span>
            <span className="cat" style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis" }}>{dir}</span>
            <button className="mv" title="移除" onClick={() => removeLocalDir(dir)}>×</button>
          </div>
        ))}
        {((s as JavSettingsWithLocal).local?.dirs ?? []).length === 0 && (
          <Ro>还没有本地文件夹 —— 点「添加本地文件夹」挑一个视频目录。</Ro>
        )}
        <div className="jvh-src-actions">
          <button className="javset-btn ghost" onClick={() => void addLocalDir()}>添加本地文件夹</button>
          <button className="javset-btn ghost" disabled={fnosBusy || !((s as JavSettingsWithLocal).local?.dirs ?? []).length}
            onClick={() => void scanLocal()}>
            {fnosBusy ? "扫描中…" : "扫描本地文件夹"}
          </button>
          {fnosNote && <span className="hint" style={{ margin: 0 }}>{fnosNote}</span>}
        </div>
      </>
    );
    return {
      sources: [{ key: "fnos", label: "飞牛媒体库", keywords: "飞牛 fnos 媒体库 nas 库 开关 刮削 本地 文件夹", el: fnosPart }],
      scan: [
        { key: "depth", label: "最大目录深度", keywords: "深度 depth 层", el: (
          <Item label="最大目录深度" path="scan.max_depth" hint="从扫描根目录往下最多翻几层。网盘上每翻一层都是一次目录枚举请求，别设太大。">
            <input type="number" min={1} max={6} value={s2.scan.maxDepth}
              onChange={(e) => patch((d) => (d.scan.maxDepth = Number(e.target.value) || 3))} />
          </Item>
        ) },
        { key: "merge", label: "合并分片文件", keywords: "分片 合并 多段 cd part", el: (
          <Bool path="scan.merge_multipart" label="合并分片文件" value={s2.scan.mergeMultipart}
            onChange={(v) => patch((d) => (d.scan.mergeMultipart = v))}
            hint="同一番号的 -1 / -2 / _part1 之类的分片会并成一个条目，播放时按段选。" />
        ) },
        { key: "prune", label: "扫描时清理失效记录", keywords: "清理 失效 prune 消失", el: (
          <Bool path="scan.prune_missing" label="扫描时清理失效记录" value={s2.scan.pruneMissing}
            onChange={(v) => patch((d) => (d.scan.pruneMissing = v))}
            hint="整库重扫后，网盘上已经不在的文件不再保留条目（不碰网盘上的任何文件）。关掉则只增不删。" />
        ) },
        { key: "exts", label: "视频扩展名", keywords: "扩展名 后缀 mp4 mkv 视频", el: (
          <Item label="视频扩展名" path="scan.video_exts" hint="逗号分隔。改错了会把非视频文件也收进库。">
            <input type="text" value={listStr(s2.scan.videoExts)} onChange={(e) => patch((d) => (d.scan.videoExts = parseList(e.target.value)))} />
          </Item>
        ) },
        { key: "skipdirs", label: "跳过的目录名", keywords: "跳过 目录 回收站 eadir", el: (
          <Item label="跳过的目录名" path="scan.skip_dirs" hint="按目录名精确跳过，大小写不敏感。系统回收站、@eaDir 缩略图缓存之类的放在这里。">
            <input type="text" value={listStr(s2.scan.skipDirs)} onChange={(e) => patch((d) => (d.scan.skipDirs = parseList(e.target.value)))} />
          </Item>
        ) },
        { key: "skippfx", label: "跳过的名称前缀", keywords: "跳过 前缀 隐藏", el: (
          <Item label="跳过的名称前缀" path="scan.skip_prefixes" hint={'默认只有 "."（隐藏项）。慎加 "@" —— 网盘资源里「@某某合集」很常见，按前缀跳会整片漏扫。'}>
            <input type="text" value={listStr(s2.scan.skipPrefixes)} onChange={(e) => patch((d) => (d.scan.skipPrefixes = parseList(e.target.value)))} />
          </Item>
        ) },
      ],
      auto: [
        { key: "autoon", label: "启用定时自动扫描", keywords: "定时 自动 扫描 周期 间隔", el: (
          <Bool path="auto_scan.enabled" label="启用定时自动扫描" value={s2.autoScan.enabled}
            onChange={(v) => patch((d) => (d.autoScan.enabled = v))}
            hint="打开后每隔一段时间自动跑一轮：重扫目录 → 清理失效 → 把待刮削队列跑一遍。默认关闭，关着时后台不会有任何动作。" />
        ) },
        { key: "interval", label: "间隔（分钟）", keywords: "间隔 分钟 周期", el: (
          <Item label="间隔（分钟，5 ~ 10080）" path="auto_scan.interval_minutes" hint="60 = 每小时，360 = 每 6 小时，1440 = 每天。网盘目录枚举是实打实的 API 请求，别设太密。">
            <input type="number" min={5} max={10080} value={s2.autoScan.intervalMinutes}
              onChange={(e) => patch((d) => (d.autoScan.intervalMinutes = Number(e.target.value) || 360))} />
          </Item>
        ) },
        { key: "scrapenew", label: "自动刮削新入库", keywords: "自动 刮削 新", el: (
          <Bool path="auto_scan.scrape_new" label="自动刮削新入库的条目" value={s2.autoScan.scrapeNew}
            onChange={(v) => patch((d) => (d.autoScan.scrapeNew = v))}
            hint="扫完顺手把「待刮削」队列跑一遍。只刮新的，不重试历史失败条目 —— 自动任务不该天天拿同一批红条目去敲站点。" />
        ) },
        { key: "cleanup", label: "清理文件已消失的条目", keywords: "清理 消失 失效", el: (
          <Bool path="auto_scan.cleanup_missing" label="清理文件已消失的条目" value={s2.autoScan.cleanupMissing}
            onChange={(v) => patch((d) => (d.autoScan.cleanupMissing = v))}
            hint="配合「扫描时清理失效记录」使用；网盘掉线导致的误清，重扫一次就会回来。" />
        ) },
        { key: "lastrun", label: "上次运行", keywords: "上次 运行 立即 跑", el: (
          <Item label="上次运行" path="auto_scan.last_run_ms">
            <div className="javset-ro" style={{ margin: 0 }}>{fmtTime(s2.autoScan.lastRunMs)}</div>
          </Item>
        ) },
        { key: "runnow", label: "立即跑一轮", keywords: "立即 跑 一轮 手动", el: (
          <button className="javset-btn ghost" disabled={autoBusy} onClick={() => void runAutoNow()}>
            {autoBusy ? "正在跑…" : "立即跑一轮"}
          </button>
        ) },
      ],
      network: [
        { key: "trustenv", label: "读取系统代理", keywords: "代理 系统 环境变量 clash", el: (
          <Bool path="network.trust_env" label="读取系统代理" value={s2.network.trustEnv}
            onChange={(v) => patch((d) => (d.network.trustEnv = v))}
            hint="默认开。环境变量（HTTP_PROXY 等）与 Windows「系统代理」注册表都会读 —— Clash / v2rayN 开系统代理时不给进程塞环境变量，这层必须读注册表才能看见。" />
        ) },
        { key: "proxy", label: "代理地址", keywords: "代理 地址 http socks 端口", el: (
          <Item label="代理地址" path="network.proxy" hint="留空 = 用上面的系统代理。填了就以这里为准（手写优先）。v2rayN 的 HTTP 端口写 http://127.0.0.1:10808，SOCKS 端口写 socks5://127.0.0.1:10808；只写 127.0.0.1:10808 也能认，会自动补 http://。">
            <input type="text" placeholder="留空 = 用上面的系统代理" value={s2.network.proxy}
              onChange={(e) => patch((d) => (d.network.proxy = e.target.value))} />
          </Item>
        ) },
        { key: "timeout", label: "请求超时（秒）", keywords: "超时 timeout 秒", el: (
          <Item label="请求超时（秒）" path="network.timeout_seconds">
            <input type="number" min={5} max={120} value={s2.network.timeoutSeconds}
              onChange={(e) => patch((d) => (d.network.timeoutSeconds = Number(e.target.value) || 25))} />
          </Item>
        ) },
        { key: "ua", label: "User-Agent", keywords: "ua user agent 浏览器", el: (
          <Item label="User-Agent" path="network.user_agent" hint="所有刮削请求都会带这个 UA。默认是一个普通的桌面 Chrome。如果某个站对你现在的 UA 特别敏感，可以换成你浏览器里那串。">
            <textarea value={s2.network.userAgent} onChange={(e) => patch((d) => (d.network.userAgent = e.target.value))} />
          </Item>
        ) },
      ],
      sites: [{ key: "sitelist", label: "站点列表", keywords: "站点 刮削 优先级 顺序 镜像 自定义 site", el: sitesPart }],
      behavior: [
        { key: "interval2", label: "请求间隔（秒）", keywords: "间隔 秒 请求", el: (
          <Item label="请求间隔（秒）" path="scrape.interval_seconds" hint="每个请求之间至少等这么久。对站点友好一点，自己也不容易被封。">
            <input type="number" min={0.2} max={60} step={0.1} value={s2.scrape.intervalSeconds}
              onChange={(e) => patch((d) => (d.scrape.intervalSeconds = Number(e.target.value) || 1.5))} />
          </Item>
        ) },
        { key: "cats", label: "参与刮削的分类", keywords: "分类 fc2 censored uncensored 有码 无码", el: (
          <Item label="参与刮削的分类" path="scrape.categories" hint="可选 fc2 / censored / uncensored。无番号的 unknown 不在其中 —— 它本来就没法按番号查。">
            <input type="text" value={listStr(s2.scrape.categories)} onChange={(e) => patch((d) => (d.scrape.categories = parseList(e.target.value)))} />
          </Item>
        ) },
        { key: "conc", label: "并发数", keywords: "并发 并行 concurrency", el: (
          <Ro>并发数暂为串行（1）—— 刮削走镜像轨 + 礼貌间隔，串行最稳；后续接入 HTML 站点引擎后再放开。</Ro>
        ) },
      ],
      ai: [
        { key: "aion", label: "启用内置 AI", keywords: "ai 启用 开", el: (
          <Bool path="ai.enabled" label="启用内置 AI（标题汉化）" value={s2.ai.enabled}
            onChange={(v) => patch((d) => (d.ai.enabled = v))}
            hint="刮削完成后把外文标题批量译成目标语言，卡片优先显示译名。兼容 OpenAI Chat Completions 协议（DeepSeek / 中转 / Ollama 均可）。" />
        ) },
        { key: "aibase", label: "接口地址", keywords: "ai 地址 base url deepseek 接口", el: (
          <Item label="接口地址（Base URL，不含 /chat/completions）" path="ai.base_url">
            <input type="text" value={s2.ai.baseUrl} onChange={(e) => patch((d) => (d.ai.baseUrl = e.target.value))} />
          </Item>
        ) },
        { key: "aikey", label: "API Key", keywords: "ai key 密钥 token", el: (
          <Item label="API Key" path="ai.api_key" hint="只存本机 jav-settings.json，不上传、不入库。">
            <input type="password" value={s2.ai.apiKey} onChange={(e) => patch((d) => (d.ai.apiKey = e.target.value))} />
          </Item>
        ) },
        { key: "aimodel", label: "模型", keywords: "模型 model deepseek", el: (
          <Item label="模型" path="ai.model">
            <input type="text" value={s2.ai.model} onChange={(e) => patch((d) => (d.ai.model = e.target.value))} />
          </Item>
        ) },
        { key: "ailang", label: "目标语言", keywords: "语言 翻译 译名 language", el: (
          <Item label="目标语言" path="ai.target_lang" hint="zh / zh-TW / en / ja。">
            <input type="text" placeholder="zh" value={s2.ai.targetLang} onChange={(e) => patch((d) => (d.ai.targetLang = e.target.value))} />
          </Item>
        ) },
      ],
      advanced: [
        { key: "paths", label: "数据目录", keywords: "目录 路径 数据 存储 json", el: (
          <Item label="数据目录" path="paths.data_dir" hint="jav-settings.json（本面板）、jav-library.json（番号条目）、cloud-library.json（网盘索引）都在这里。">
            <div className="javset-ro" style={{ margin: 0 }}>{s2.paths?.dataDir || "（未知）"}</div>
          </Item>
        ) },
        { key: "todo", label: "待接入", keywords: "待接入 后续 javbus javdb", el: (
          <Ro>JavBus / JavDB / metatube / 自定义站点引擎、STRM 导出、网盘删除暂未接入 —— 接入后对应设置项会出现在这里。</Ro>
        ) },
      ],
    };
  };

  /* 搜索：跨节匹配字段标题/关键词，命中字段平铺（对照原版「搜设置项」） */
  const query = q.trim().toLowerCase();
  const all = s ? buildSections(s) : null;
  const matched = s && all && query
    ? SECTIONS.flatMap((x) => all[x.key].filter((f) => f.label.toLowerCase().includes(query) || f.keywords.includes(query)).map((f) => ({ sec: x.label, f })))
    : [];
  const secDesc = sec === "sources" ? "网盘与飞牛媒体库" : undefined;

  return (
    <div className="javset embed">
      {/* 顶部横排分节导航（原版为左侧竖排） */}
        <div className="javset-tabs">
          {SECTIONS.map((x) => {
            const Ico = x.icon;
            return (
              <button key={x.key} type="button" title={x.label}
                className={`javset-tab${sec === x.key && !query ? " on" : ""}`}
                onClick={() => { setSec(x.key); setQ(""); }}>
                <Ico size={15} strokeWidth={1.9} />
                <span className="t">{x.label}</span>
              </button>
            );
          })}
        </div>
        {/* 搜索条（照抄 setsearch） */}
        <div className="javset-search">
          <input type="search" placeholder="搜设置项，比如「代理」「超时」「分片」…" value={q} onChange={(e) => setQ(e.target.value)} />
          {query && <span className="cnt">{matched.length} 项</span>}
        </div>
        <div className="javset-fields" key={sec}>
          {!s && <Ro>加载中…</Ro>}
          {s && query && matched.length === 0 && <Ro>没有匹配「{q}」的设置项。</Ro>}
          {s && query && matched.map(({ sec: sl, f }) => (
            <div key={sl + f.key}>
              {f.el}
              <div style={{ fontSize: "var(--fs-cap)", color: "var(--muted)", margin: "-4px 2px 10px" }}>{sl}</div>
            </div>
          ))}
          {s && all && !query && (
            <>
              <div className="javset-sechead">
                <h4>{SECTIONS.find((x) => x.key === sec)?.label}</h4>
                {secDesc && <p>{secDesc}</p>}
              </div>
              {all[sec].map((f) => <div key={f.key}>{f.el}</div>)}
            </>
          )}
        </div>
        <div className="javset-foot">
          <span className="dirty">{note || (dirty ? "有未保存的改动" : "")}</span>
          <button className="javset-btn" disabled={!s || !dirty || saving} onClick={() => void save()}>
            {saving ? "保存中…" : "保存"}
          </button>
        </div>
    </div>
  );
}
