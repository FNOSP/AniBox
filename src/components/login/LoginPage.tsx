/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。
   [lc-2042] 登录页精简：去掉「网盘账号」扫码卡（网盘接入统一走应用内「云盘接入」页），
   只留 飞牛 NAS 一张卡（网盘与 WebDAV 均走应用内「云盘接入」页）。 */
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Config } from "../../types";

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
   登录页只保留飞牛 NAS 一张来源卡，不接也能先进首页。
   落盘沿用既有存储（config.json），
   登录页只做编排不造新存储。[lc-2042] 网盘扫码卡已移除。 */

/* [lc-1993] `notice`：会话失效被送到登录页时的一次性提示（如「登录已过期，请重新登录」）。
   与页面自己的 `err` 分开：用户一提交/切换来源就消失，不参与错误状态。 */
export function LoginPage({
  onDone,
  onGuest,
  notice,
}: {
  onDone: () => void;
  onGuest?: () => void;
  notice?: string;
}) {
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
  const [openCard, setOpenCard] = useState<"fnos" | null>("fnos");

  const alive = useRef(true);

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
    });
    return () => {
      alive.current = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 填的是 FN ID 或中继域名 → 走 FN ID 授权（中继域名不支持密码登录，见 relayFnId 注释）。
  const fnidVal = isFnId(domain.trim()) ? domain.trim() : relayFnId(domain);
  const fnosMode = !!domain.trim(); // 用户打算接飞牛
  const fnosReady = fnosMode && (!!fnidVal || (!!username && !!password));

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    // 没填飞牛 → 「开始使用」：直接进首页（游客态 + 已接入的网盘照常可用）
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

  return (
    <div className="login-wrap">
      <LoginBackdrop />
      <div className="login-hero">
        <div className="login-hero-glow" />
        <h1>AniBox</h1>
        <p>把你的影视库，变成桌面上的私人影院</p>
        <ul className="login-feats">
          <li>夸克 / 百度 / 阿里 / 123 / 115 扫码即接，WebDAV 与 Alist 通吃</li>
          <li>飞牛影视原生支持：媒体库、追剧进度与播放状态双向同步</li>
          <li>mpv 与 PotPlayer 播放，字幕、着色器与插帧原生可用</li>
          <li>聚合 TMDB / Bangumi / AniList 等丰富影视数据</li>
        </ul>
      </div>
      <form className="login-card" onSubmit={submit}>
        <h2>选择内容来源</h2>
        <p className="login-src-sub">按需组合接入；一个都不接也可以先进去看看</p>

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

        {err ? <div className="err">{err}</div> : notice ? <div className="err">{notice}</div> : null}
        <div className="login-btnrow">
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
          {/* [lc-2045] 游客入口独立成键：填了服务器地址时主按钮是「登录」，
              原先「先不登录」只在地址留空时才出现，用户找不到 */}
          <button type="button" className="ghost" disabled={busy} onClick={() => (onGuest ? onGuest() : onDone())}>
            游客模式
          </button>
        </div>
      </form>
    </div>
  );
}

/* ────────────────────────── 首页全屏海报轮播（复刻 emby-crx Misty banner） ────────────────────────── */
