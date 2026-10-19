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

export function LoginPage({ onDone, onGuest }: { onDone: () => void; onGuest?: () => void }) {
  const [cfg, setCfg] = useState<Config | null>(null);
  const [server, setServer] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  // [访问码] fnOS 网关「应用访问码」：可选，留空表示服务器未开启门禁。
  // 与旧项目 Fntv-Plus 一致：非空时登录前先向 /access_code_verify 验证并建立网关会话。
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

  useEffect(() => {
    invoke<Config>("get_config").then((c) => {
      setCfg(c);
      setServer(c.server);
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
    setBusy(true);
    setErr("");
    try {
      await invoke("save_config", {
        config: {
          ...cfg,
          server: server.trim(),
          username,
          password: remember ? password : "",
          access_code: accessCode.trim(),
          auto_login: remember && autoLogin,
          mpv_path: mpvPath,
          proxy_path: proxyPath,
        },
      });
      await invoke("login", {
        server: server.trim(),
        username,
        password,
        accessCode: accessCode.trim(),
      });
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
        <label>服务器地址</label>
        <input
          value={server}
          onChange={(e) => setServer(e.target.value)}
          placeholder="如 http://192.168.31.170:18888 或 100.66.1.2:18888"
          required
        />
        <label>用户名</label>
        <input value={username} onChange={(e) => setUsername(e.target.value)} required />
        <label>密码</label>
        <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} required />
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
        <button className="primary" disabled={busy}>
          {busy ? "登录中…" : "登 录"}
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
