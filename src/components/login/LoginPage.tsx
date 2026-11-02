/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { RefreshCw as IcoRefreshL, Trash2 as IcoTrashL } from "lucide-react";
import { Config } from "../../types";
import { useCloudQrLogin } from "../../hooks/useCloudQrLogin";

/* [lc-17xx] 登录页背景：public/login/ 下的横图随机轮换。
   · 首帧随机选一张，之后每 20s 交叉淡入淡出切换；
   · prefers-reduced-motion 下只保留首帧、不自动切换；
   · 只用仓库内图片（无外部 URL），遮罩由 10-login.css 的主题变量控制。 */
const LOGIN_BACKGROUNDS = ["/login/bg-login.webp", "/login/bg-login.jpg"] as const;
const LOGIN_BG_INTERVAL_MS = 20000;

function LoginBackdrop() {
  const [active, setActive] = useState(() =>
    Math.floor(Math.random() * LOGIN_BACKGROUNDS.length),
  );

  useEffect(() => {
    if (LOGIN_BACKGROUNDS.length < 2) return;
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const timer = window.setInterval(() => {
      setActive((i) => (i + 1) % LOGIN_BACKGROUNDS.length);
    }, LOGIN_BG_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, []);

  return (
    <div className="login-bg" aria-hidden="true">
      {LOGIN_BACKGROUNDS.map((src, i) => (
        <div
          key={src}
          className={`login-bg-img${i === active ? " active" : ""}`}
          style={{ backgroundImage: `url("${src}")` }}
        />
      ))}
      <div className="login-bg-veil" />
    </div>
  );
}

/* [lc-1884] 回归原项目「不区分登录方式」：一个输入框同时接受 IP/域名/FN ID，
   后端按格式自动选择「本地账号登录」还是「FN ID 远程登录」。
   判定与原项目 `resource/login/index.html` 的 isFnId 完全一致：
   不含点号、长度 6~30 —— 符合即当 FN ID，否则当服务器地址。 */
function isFnId(value: string): boolean {
  return !!value && !value.includes(".") && value.length >= 6 && value.length <= 30;
}

/* [lc-1885] 飞牛官方中继域名（{fnId}.fnos.net / *.5ddd.com）**不支持账号密码登录**：
   实测同一账号在直连 IP 上 `POST /v/api/v1/login` 返回 code=0，但在中继域名上返回
   `-15 Password Incorrect`（中继要求走 FN ID 授权/OAuth，而非影视密码接口）。
   因此只要用户填的是中继域名，就自动改用 FN ID 授权流程，避免撞 -15。 */
function relayFnId(value: string): string {
  const host = value
    .trim()
    .replace(/^https?:\/\//i, "")
    .split("/")[0]
    .split(":")[0]
    .toLowerCase();
  if (host.endsWith(".fnos.net")) {
    const sub = host.slice(0, -".fnos.net".length);
    if (sub && !sub.includes(".")) return sub;
  }
  return "";
}

/* [lc-1889] 网盘优先：登录页从「登录飞牛」改为「选择内容来源」——
   网盘扫码 / 飞牛 NAS / WebDAV 三张来源卡任意组合，都不接也能先进首页。
   各来源的落盘沿用既有存储（config.json / direct-cloud-auth.json /
   cloud115.json / webdav-accounts.json），登录页只做编排不造新存储。 */
type ProviderInfo = { id: string; label: string; logged: boolean; userName: string };
type WebdavAccount = { id: string; name: string; url: string; username: string };

export function LoginPage({ onDone, onGuest }: { onDone: () => void; onGuest?: () => void }) {
  const [cfg, setCfg] = useState<Config | null>(null);
  // [lc-1884] 单一输入框：IP / 域名 / FN ID 都填这里，提交时自动判别。
  const [domain, setDomain] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  // [访问码] fnOS 网关「应用访问码」：可选，留空表示服务器未开启门禁。
  const [accessCode, setAccessCode] = useState("");
  const [remember, setRemember] = useState(false);
  const [autoLogin, setAutoLogin] = useState(false);
  const [mpvPath, setMpvPath] = useState("");
  const [proxyPath, setProxyPath] = useState("");
  const [showAdv, setShowAdv] = useState(false);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  // 自动登录一次性闸门：失败后停在本页报错，绝不循环重试
  const autoTried = useRef(false);

  // ── 来源卡折叠态 ──
  const [openCard, setOpenCard] = useState<"cloud" | "fnos" | "webdav" | null>("cloud");

  // ── 网盘来源（扫码） ──
  const [providers, setProviders] = useState<ProviderInfo[]>([]);
  const [activeProv, setActiveProv] = useState("");
  const alive = useRef(true);

  // ── WebDAV 来源 ──
  const [wdAccounts, setWdAccounts] = useState<WebdavAccount[]>([]);
  const [wdForm, setWdForm] = useState<{ name: string; url: string; user: string; pass: string } | null>(null);
  const [wdErr, setWdErr] = useState("");

  const refreshProviders = useCallback(async (): Promise<ProviderInfo[]> => {
    const r = await invoke<ProviderInfo[]>("cloud_providers");
    if (alive.current) setProviders(r);
    return r;
  }, []);

  const loadWdAccounts = useCallback(async () => {
    try {
      const r = await invoke<WebdavAccount[]>("webdav_accounts");
      if (alive.current) setWdAccounts(r);
      return r;
    } catch {
      return [];
    }
  }, []);

  // [lc-1889] 内嵌扫码：与 CloudPanel 共用 useCloudQrLogin（串行轮询 + 到点自动换码），
  // 成功后只刷新登录态，不进入文件浏览（浏览仍在「网盘接入」页）。
  const qrLogin = useCloudQrLogin(() => {
    void refreshProviders();
  });
  /** 重新取码前清掉页面级报错（旧报错和新二维码同屏会误导） */
  const startQr = useCallback(
    (provider: string) => {
      setErr("");
      void qrLogin.start(provider);
    },
    [qrLogin.start],
  );

  const pickProv = (id: string) => {
    if (id === activeProv) return;
    qrLogin.stop();
    setActiveProv(id);
    const p = providers.find((x) => x.id === id);
    if (!p?.logged) void startQr(id);
  };

  const addWdAccount = async () => {
    if (!wdForm) return;
    setWdErr("");
    try {
      await invoke("webdav_account_add", { name: wdForm.name, url: wdForm.url, username: wdForm.user, password: wdForm.pass });
      setWdForm(null);
      void loadWdAccounts();
    } catch (ex) {
      setWdErr(String(ex));
    }
  };

  const delWdAccount = async (id: string) => {
    await invoke("webdav_account_del", { id }).catch(() => {});
    void loadWdAccounts();
  };

  useEffect(() => {
    alive.current = true;
    invoke<Config>("get_config").then((c) => {
      if (!alive.current) return;
      setCfg(c);
      setDomain(c.server);
      setUsername(c.username);
      setPassword(c.password || "");
      setAccessCode(c.access_code || "");
      setRemember(!!c.password);
      setAutoLogin(c.auto_login && !!c.password);
      setMpvPath(c.mpv_path);
      setProxyPath(c.proxy_path);
      // 有飞牛配置时默认展开 NAS 卡，否则默认展示网盘卡
      if (c.server) setOpenCard("fnos");
    });
    refreshProviders().then((list) => {
      if (!alive.current || !list.length) return;
      setActiveProv(list[0].id);
      if (!list[0].logged) void startQr(list[0].id);
    });
    void loadWdAccounts();
    return () => {
      alive.current = false;
      qrLogin.stop();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 填的是 FN ID 或中继域名 → 走 FN ID 授权（中继域名不支持密码登录，见 relayFnId 注释）。
  const fnidVal = isFnId(domain.trim()) ? domain.trim() : relayFnId(domain);
  const fnosMode = !!domain.trim(); // 用户打算接飞牛
  const fnosReady = fnosMode && (!!fnidVal || (!!username && !!password));
  const loggedClouds = providers.filter((p) => p.logged);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    // 没填飞牛 → 「开始使用」：直接进首页（游客态 + 已接入的网盘/WebDAV 照常可用）
    if (!fnosMode) {
      if (onGuest) onGuest();
      else onDone();
      return;
    }
    if (!fnosReady) {
      setErr("请输入用户名和密码。");
      return;
    }
    setBusy(true);
    setErr("");
    try {
      if (fnidVal) {
        // FN ID 远程登录：打开飞牛官方授权窗口（OAuth），成功后后端已写配置。
        await invoke("fn_id_login", { fnId: fnidVal, accessCode: accessCode.trim() });
        try { localStorage.setItem("anibox-fnid", fnidVal); } catch { /* 尽力预填 */ }
      } else {
        // 本地账号登录：先登录（成功后 login 已持久化 server/token/access_code），
        // 再回写登录语义字段，避免拿挂载时旧快照把解析出的端口和 token 冲回旧值。
        await invoke("login", {
          server: domain.trim(),
          username,
          password,
          accessCode: accessCode.trim(),
        });
        const fresh = await invoke<Config>("get_config");
        await invoke("save_config", {
          config: {
            ...fresh,
            username,
            password: remember ? password : "",
            auto_login: remember && autoLogin,
            mpv_path: mpvPath,
            proxy_path: proxyPath,
          },
        });
      }
      onDone();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  }

  // 启动自动登录：token 已由 check_session 把过关，走到登录页即代表失效；
  // 记住了密码且开了自动登录 → 用记住的凭据静默重登一次（成功直接进首页）
  useEffect(() => {
    if (autoTried.current || !cfg) return;
    if (!cfg.auto_login || !cfg.password || !cfg.server || !cfg.username) return;
    autoTried.current = true;
    setBusy(true);
    invoke("login", {
      server: cfg.server,
      username: cfg.username,
      password: cfg.password,
      accessCode: cfg.access_code || "",
    })
      .then(() => onDone())
      .catch((e) => {
        setErr("自动登录失败：" + String(e));
        setBusy(false);
      });
  }, [cfg, onDone]);

  const curProv = providers.find((p) => p.id === activeProv);

  return (
    <div className="login-wrap">
      <LoginBackdrop />
      <div className="login-hero">
        <div className="login-hero-glow" />
        <h1>AniBox</h1>
        <p>把你的影视库，变成桌面上的私人影院</p>
        <ul className="login-feats">
          <li>夸克 / 百度 / 阿里 / 天翼 / 123 / 115 扫码即接，WebDAV 通吃 NAS 与 Alist</li>
          <li>飞牛影视原生支持：媒体库、追剧进度与播放状态双向同步</li>
          <li>mpv 与 PotPlayer 播放，字幕、着色器与插帧原生可用</li>
          <li>聚合 TMDB / Bangumi / AniList 等丰富影视数据</li>
        </ul>
      </div>
      <form className="login-card" onSubmit={submit}>
        <h2>选择内容来源</h2>
        <p className="login-src-sub">按需组合接入；一个都不接也可以先进去看看</p>

        {/* ── 来源卡：网盘账号 ── */}
        <section className={`login-src${openCard === "cloud" ? " open" : ""}`}>
          <button
            type="button"
            className="login-src-head"
            aria-expanded={openCard === "cloud"}
            onClick={() => setOpenCard(openCard === "cloud" ? null : "cloud")}
          >
            网盘账号
            {loggedClouds.length > 0 && <span className="login-src-badge">{loggedClouds.length}</span>}
            <span className="login-src-chev" aria-hidden="true" />
          </button>
          {openCard === "cloud" && (
            <div className="login-src-body">
              <div className="login-src-chips" role="tablist">
                {providers.map((p) => (
                  <button
                    key={p.id}
                    type="button"
                    role="tab"
                    aria-selected={p.id === activeProv}
                    className={`login-chip${p.id === activeProv ? " on" : ""}`}
                    onClick={() => pickProv(p.id)}
                  >
                    {p.label}
                    <span className={`login-chip-dot${p.logged ? " ok" : ""}`} title={p.logged ? "已登录" : "未登录"} />
                  </button>
                ))}
              </div>
              {curProv?.logged ? (
                <p className="login-src-ok">已登录{curProv.userName ? `：${curProv.userName}` : ""}</p>
              ) : qrLogin.image ? (
                <div className="login-qrrow">
                  <img className="login-qr" src={qrLogin.image} alt="登录二维码" />
                  <div className="login-qrcol">
                    <p className={`login-qrmsg${qrLogin.phase === "error" ? " err" : ""}`}>
                      {qrLogin.message}
                      {qrLogin.phase !== "success" && qrLogin.secondsLeft > 0 ? ` · ${qrLogin.secondsLeft}s 后自动刷新` : ""}
                    </p>
                    <button
                      type="button"
                      className="login-minibtn"
                      onClick={() => void startQr(activeProv)}
                      disabled={qrLogin.phase === "loading"}
                    >
                      <IcoRefreshL size={12} strokeWidth={2} /> 刷新二维码
                    </button>
                    <p className="login-dim">凭据只保存在本机，不会上传到任何服务器。</p>
                  </div>
                </div>
              ) : (
                <p className={`login-dim${qrLogin.phase === "error" ? " err" : ""}`}>
                  {qrLogin.message || "正在获取二维码…"}
                </p>
              )}
            </div>
          )}
        </section>

        {/* ── 来源卡：飞牛 NAS ── */}
        <section className={`login-src${openCard === "fnos" ? " open" : ""}`}>
          <button
            type="button"
            className="login-src-head"
            aria-expanded={openCard === "fnos"}
            onClick={() => setOpenCard(openCard === "fnos" ? null : "fnos")}
          >
            飞牛 NAS
            {cfg?.token ? <span className="login-src-badge">已登录</span> : null}
            <span className="login-src-chev" aria-hidden="true" />
          </button>
          {openCard === "fnos" && (
            <div className="login-src-body">
              <label>服务器地址 / FN ID</label>
              <input
                value={domain}
                onChange={(e) => setDomain(e.target.value)}
                placeholder="IP、域名或 FN ID，如 192.168.31.170:18888"
                spellCheck={false}
              />
              <p className="login-dim" style={{ margin: "4px 0 0" }}>
                填内网 IP / 直连域名用账号密码登录；填 FN ID 或中继域名（*.fnos.net）则打开飞牛官方授权窗口远程登录。
              </p>
              <label>用户名</label>
              <input value={username} onChange={(e) => setUsername(e.target.value)} placeholder={fnidVal ? "FN ID 登录时可留空" : ""} />
              <label>密码</label>
              <input
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                placeholder={fnidVal ? "FN ID 登录时可留空" : ""}
              />
              <label>访问码（可选）</label>
              <input
                type="password"
                value={accessCode}
                onChange={(e) => setAccessCode(e.target.value)}
                placeholder="服务器未开启访问码门禁时请留空"
                autoComplete="off"
              />
              <div className="login-opts">
                <label className="login-check">
                  <input
                    type="checkbox"
                    checked={remember}
                    onChange={(e) => {
                      setRemember(e.target.checked);
                      if (!e.target.checked) setAutoLogin(false);
                    }}
                  />
                  记住密码
                </label>
                <label className="login-check">
                  <input
                    type="checkbox"
                    checked={autoLogin}
                    disabled={!remember}
                    onChange={(e) => setAutoLogin(e.target.checked)}
                  />
                  自动登录
                </label>
              </div>
              <button type="button" className="linklike" onClick={() => setShowAdv(!showAdv)}>
                高级设置（mpv / 代理路径）
              </button>
              {showAdv && (
                <>
                  <label>mpv.exe 路径</label>
                  <input value={mpvPath} onChange={(e) => setMpvPath(e.target.value)} />
                  <label>Go 代理路径</label>
                  <input value={proxyPath} onChange={(e) => setProxyPath(e.target.value)} />
                </>
              )}
            </div>
          )}
        </section>

        {/* ── 来源卡：WebDAV ── */}
        <section className={`login-src${openCard === "webdav" ? " open" : ""}`}>
          <button
            type="button"
            className="login-src-head"
            aria-expanded={openCard === "webdav"}
            onClick={() => setOpenCard(openCard === "webdav" ? null : "webdav")}
          >
            WebDAV
            {wdAccounts.length > 0 && <span className="login-src-badge">{wdAccounts.length}</span>}
            <span className="login-src-chev" aria-hidden="true" />
          </button>
          {openCard === "webdav" && (
            <div className="login-src-body">
              {wdAccounts.map((a) => (
                <div key={a.id} className="login-wdrow">
                  <span className="login-wdmeta">
                    <b>{a.name}</b>
                    <span>{a.url}</span>
                  </span>
                  <button type="button" className="login-minibtn" title="删除账号" onClick={() => void delWdAccount(a.id)}>
                    <IcoTrashL size={12} strokeWidth={2} />
                  </button>
                </div>
              ))}
              {wdForm ? (
                <div className="login-wdform">
                  <input placeholder="名称（如：家里 NAS）" value={wdForm.name} onChange={(e) => setWdForm({ ...wdForm, name: e.target.value })} />
                  <input placeholder="地址 https://dav.example.com/dav" value={wdForm.url} onChange={(e) => setWdForm({ ...wdForm, url: e.target.value })} />
                  <input placeholder="账号" value={wdForm.user} onChange={(e) => setWdForm({ ...wdForm, user: e.target.value })} />
                  <input placeholder="密码" type="password" value={wdForm.pass} onChange={(e) => setWdForm({ ...wdForm, pass: e.target.value })} />
                  <div className="login-wdbtns">
                    <button type="button" className="login-minibtn" onClick={() => void addWdAccount()}>
                      验证并保存
                    </button>
                    <button type="button" className="login-minibtn" onClick={() => setWdForm(null)}>
                      取消
                    </button>
                  </div>
                </div>
              ) : (
                <button type="button" className="login-minibtn" onClick={() => setWdForm({ name: "", url: "", user: "", pass: "" })}>
                  添加 WebDAV 账号
                </button>
              )}
              {wdErr && <p className="login-err-dim">{wdErr}</p>}
              <p className="login-dim">支持 NAS / Alist / 坚果云等任意 WebDAV 服务；密码本机加密存储。</p>
            </div>
          )}
        </section>

        {err && <div className="err">{err}</div>}
        <button className="primary" disabled={busy}>
          {busy
            ? fnosMode
              ? fnidVal
                ? "打开授权窗口…"
                : "登录中…"
              : "进入中…"
            : fnosMode
              ? fnidVal
                ? "打开飞牛授权窗口"
                : "登 录"
              : "开始使用"}
        </button>
        {!fnosMode && (
          <p className="login-dim" style={{ textAlign: "center", marginTop: -6 }}>
            未接飞牛时以游客模式进入，已接入的网盘 / WebDAV 在「网盘接入」里浏览。
          </p>
        )}
      </form>
    </div>
  );
}

/* ────────────────────────── 首页全屏海报轮播（复刻 emby-crx Misty banner） ────────────────────────── */
