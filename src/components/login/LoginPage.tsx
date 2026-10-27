/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
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

  // 填的是 FN ID 或中继域名 → 走 FN ID 授权（中继域名不支持密码登录，见 relayFnId 注释）。
  const fnidVal = isFnId(domain.trim()) ? domain.trim() : relayFnId(domain);
  const fnid = !!fnidVal;
  // FN ID 走 OAuth，不需要用户名/密码；服务器地址登录则两者必填。
  const canSubmit = !!domain.trim() && (fnid || (!!username && !!password));

  useEffect(() => {
    invoke<Config>("get_config").then((c) => {
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
  }, []);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!domain.trim()) {
      setErr("请输入服务器地址或 FN ID。");
      return;
    }
    if (!fnid && (!username || !password)) {
      setErr("请输入用户名和密码。");
      return;
    }
    setBusy(true);
    setErr("");
    try {
      if (fnid) {
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
        <p>把飞牛影视，变成你桌面上的私人影院</p>
        <ul className="login-feats">
          <li>自绘界面持续优化，本地媒体库、收藏与继续观看一目了然</li>
          <li>支持 mpv 与 PotPlayer 播放，字幕、着色器与播放控制原生可用</li>
          <li>追剧日历轻松追踪，放送时间与观看进度不再错过</li>
          <li>聚合 TMDB / Bangumi / AniList 等丰富影视数据</li>
          <li>弹幕、全局手柄与自建镜像，完善的追剧观影体验</li>
        </ul>
      </div>
      <form className="login-card" onSubmit={submit}>
        <h2>登录飞牛影视</h2>
        <label>服务器地址 / FN ID</label>
        <input
          value={domain}
          onChange={(e) => setDomain(e.target.value)}
          placeholder="IP、域名或 FN ID，如 192.168.31.170:18888"
          spellCheck={false}
          required
        />
        <p className="set-desc" style={{ margin: "4px 0 0", fontSize: 12 }}>
          填内网 IP / 直连域名用账号密码登录；填 FN ID 或中继域名（*.fnos.net）则打开飞牛官方授权窗口远程登录。
        </p>
        <label>用户名</label>
        <input value={username} onChange={(e) => setUsername(e.target.value)} placeholder={fnid ? "FN ID 登录时可留空" : ""} />
        <label>密码</label>
        <input
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          placeholder={fnid ? "FN ID 登录时可留空" : ""}
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
        {err && <div className="err">{err}</div>}
        <button className="primary" disabled={busy || !canSubmit}>
          {busy ? (fnid ? "打开授权窗口…" : "登录中…") : "登 录"}
        </button>
        {/* [lc-1600] 游客入口：未登录也能用（首页轮播走 AniList 当季 + TMDB 热门兜底；
            详情/播放引导登录）。标记写进 localStorage，启动流程据此跳过会话检查。 */}
        {onGuest && (
          <button
            type="button"
            className="gst-enter"
            disabled={busy}
            onClick={() => {
              try {
                localStorage.setItem("anibox-guest", "1");
              } catch {
                /* 写不进也不拦：本次会话照常进游客态 */
              }
              onGuest();
            }}
          >
            先逛逛（游客模式）
          </button>
        )}
      </form>
    </div>
  );
}

/* ────────────────────────── 首页全屏海报轮播（复刻 emby-crx Misty banner） ────────────────────────── */
