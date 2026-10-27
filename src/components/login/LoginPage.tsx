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
  // [lc-1851] 登录方式：account=账号密码（原样） / fnid=FN ID 远程登录（旧项目 Fntv-Plus
  // 3.x 已有、AniBox 移植回来——出差/外网场景经飞牛官方 FN Connect 中继连回家）
  const [mode, setMode] = useState<"account" | "fnid">("account");
  const [fnId, setFnId] = useState(() => {
    try { return localStorage.getItem("anibox-fnid") || ""; } catch { return ""; }
  });
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
    if (!password) {
      setErr("请输入密码。");
      return;
    }
    setBusy(true);
    setErr("");
    try {
      // 先登录：login 成功后会持久化 server（含解析出的真实端口）/ token / access_code。
      await invoke("login", {
        server: server.trim(),
        username,
        password,
        accessCode: accessCode.trim(),
      });
      // 登录成功后再回写登录语义字段（记住密码 / 自动登录 / 高级路径）。
      // 用 get_config 取回刚由 login 写入的 server（真实基址）与 token，避免拿挂载时的
      // 旧快照把解析出的端口和 token 冲回旧值；登录失败则不写盘 —— 保证「登录失败不抹掉
      // 已记住的密码」，也避免旧版把 config.server 写成裸 IP 的坑。
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
      onDone();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  }

  // [lc-1851] FN ID 登录：后端打开 OAuth 窗口走 FN Connect（5ddd.com → NAS 授权 →
  // code 换 token），成功即已写配置，直接 onDone。FN ID 记 localStorage 供下次预填。
  async function submitFnId(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setErr("");
    try {
      await invoke("fn_id_login", { fnId: fnId.trim(), accessCode: accessCode.trim() });
      try { localStorage.setItem("anibox-fnid", fnId.trim()); } catch { /* 尽力预填 */ }
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
      <form className="login-card" onSubmit={mode === "account" ? submit : submitFnId}>
        <h2>登录飞牛影视</h2>
        {/* [lc-1851] 登录方式切换：账号密码（局域网/直连） / FN ID（远程，经飞牛官方中继） */}
        <div className="login-opts" style={{ marginBottom: 8 }}>
          <label className="login-check" style={{ marginRight: 16 }}>
            <input
              type="radio"
              name="login-mode"
              checked={mode === "account"}
              onChange={() => setMode("account")}
            />
            账号密码登录
          </label>
          <label className="login-check">
            <input
              type="radio"
              name="login-mode"
              checked={mode === "fnid"}
              onChange={() => setMode("fnid")}
            />
            FN ID 远程登录
          </label>
        </div>
        {mode === "fnid" ? (
          <>
            <label>FN ID（飞牛后台 FN Connect 页获取）</label>
            <input
              value={fnId}
              onChange={(e) => setFnId(e.target.value)}
              placeholder="FN Connect 里给你的那串 ID"
              required
            />
            <label>访问码（可选）</label>
            <input
              type="password"
              value={accessCode}
              onChange={(e) => setAccessCode(e.target.value)}
              placeholder="NAS 开启访问码门禁时填写"
              autoComplete="off"
            />
            <p className="set-desc" style={{ margin: "4px 0 0", fontSize: 12 }}>
              将打开飞牛官方授权窗口：登录你的飞牛 ID 并点「授权」即可，无需知道 NAS 地址。
              中继首次连接可能较慢（最长约 4 分钟）。
            </p>
          </>
        ) : (
          <>
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
            </>
        )}
        {mode === "account" && (
          <>
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
          </>
        )}
        {err && <div className="err">{err}</div>}
        <button className="primary" disabled={busy}>
          {busy ? "登录中…" : mode === "fnid" ? "打开飞牛授权窗口" : "登 录"}
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
