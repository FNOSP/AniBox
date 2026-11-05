// Package security —— 端口直连模式的访问密码门禁。
//
// fpk 改为端口直连后（lc-2032），服务不再依赖 fnOS 统一网关的 NAS 登录态，
// 局域网内任何设备都能直达增强页与管理 API。本包提供可选的「访问密码」门禁：
//
//	设置面板开启 + 设密码 → 全部路由（除验证接口本身）要求持有效会话 Cookie；
//	未验证的浏览器导航请求 → 返回内嵌密码页（无外部依赖，输完即进）；
//	未验证的 API/资源请求 → 401（前端 fetch 拿到状态码自行处理）。
//
// 会话无状态：Cookie 值 = <过期时间>|<HMAC-SHA256(会话密钥, 过期时间|密码哈希)>。
// 签名绑定当前密码哈希——改密码即全端下线；会话密钥随机生成落 var 目录，
// 重启后仍有效；密码或密钥任一变化，旧 Cookie 全部失效。
// 防爆破：按来源 IP 记失败次数，连续 10 次失败锁 60 秒（内存态，重启即清）。
//
// 兜底：securityEnabled 开着但密码为空 = 门禁不生效（防「设了开关忘设密码」
// 把自己锁死在外面）；忘记密码可 SSH 删 config.json 的 securityPassword 字段重启。
package security

import (
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/hex"
	"encoding/json"
	"io"
	"log"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"

	"fntvplus/internal/config"
)

const (
	// CookieName 是会话 Cookie 名。
	CookieName = "fntv_auth"
	// AuthPath 是密码验证接口（门禁唯一豁免路由）。
	AuthPath = "/app/fntvplus/api/auth"
	// sessionDays 是验证通过后的免输天数。
	sessionDays = 30
	// maxFails 与 lockout 组成按 IP 的防爆破闸。
	maxFails  = 10
	lockout   = time.Minute
	secretKey = "auth_secret.key"
)

// Gate 持有门禁状态：配置、会话密钥、失败计数。
type Gate struct {
	cfg    *config.Config
	varDir string

	mu     sync.Mutex
	secret []byte
	fails  map[string]*failTrack
}

type failTrack struct {
	count       int
	lockedUntil time.Time
	lastFail    time.Time
}

// New 构造 Gate 并加载（或创建）会话密钥。
func New(cfg *config.Config, varDir string) *Gate {
	g := &Gate{cfg: cfg, varDir: varDir, fails: map[string]*failTrack{}}
	g.loadSecret()
	return g
}

// loadSecret 读 var/auth_secret.key（hex 字符串）；没有则生成 32 字节随机值落盘。
// 密钥丢失 = 全部会话失效（重新输一次密码而已），不算事故。
func (g *Gate) loadSecret() {
	path := filepath.Join(g.varDir, secretKey)
	if data, err := os.ReadFile(path); err == nil {
		if s := strings.TrimSpace(string(data)); len(s) >= 32 {
			g.secret = []byte(s)
			return
		}
	}
	b := make([]byte, 32)
	if _, err := rand.Read(b); err != nil {
		log.Printf("[security] 生成会话密钥失败: %v（本次进程内验证仍可用，重启后会话失效）", err)
		return
	}
	s := hex.EncodeToString(b)
	if err := os.WriteFile(path, []byte(s), 0o600); err != nil {
		log.Printf("[security] 会话密钥落盘失败: %v", err)
	}
	g.secret = []byte(s)
}

// Enabled 门禁是否实际生效：开关打开 **且** 已设非空密码。
func (g *Gate) Enabled() bool {
	if on, _ := g.cfg.GetSetting("securityEnabled"); on != "1" {
		return false
	}
	pw, _ := g.cfg.GetSetting("securityPassword")
	return strings.TrimSpace(pw) != ""
}

// pwdHash 当前密码的绑定哈希（只在本服务端内存/签名里用，不下发不落盘）。
// 密码为空时返回空串——空密码的门禁本就不生效，签名校验时不会走到。
func (g *Gate) pwdHash() string {
	pw, _ := g.cfg.GetSetting("securityPassword")
	sum := sha256.Sum256([]byte("fntv-auth|" + pw))
	return hex.EncodeToString(sum[:])
}

// mac 计算 exp|pwdHash 的 HMAC-SHA256 十六进制串（签名本体）。
func (g *Gate) mac(exp int64) string {
	mac := hmac.New(sha256.New, g.secret)
	io.WriteString(mac, "fntv-auth|")
	io.WriteString(mac, strconv.FormatInt(exp, 10))
	io.WriteString(mac, "|")
	io.WriteString(mac, g.pwdHash())
	return hex.EncodeToString(mac.Sum(nil))
}

// sign 生成 <exp>|<mac> 形态的会话令牌。
func (g *Gate) sign(exp int64) string {
	return strconv.FormatInt(exp, 10) + "|" + g.mac(exp)
}

// verifyToken 校验 Cookie 值：格式、有效期、签名（绑定当前密码哈希与密钥）。
func (g *Gate) verifyToken(value string) bool {
	expStr, sig, ok := strings.Cut(value, "|")
	if !ok {
		return false
	}
	exp, err := strconv.ParseInt(expStr, 10, 64)
	if err != nil || time.Now().Unix() > exp {
		return false
	}
	if g.secret == nil {
		return false
	}
	return subtle.ConstantTimeCompare([]byte(sig), []byte(g.mac(exp))) == 1
}

// Wrap 给整棵路由树套上门禁。authPath 豁免（否则连验证接口都进不去）。
func (g *Gate) Wrap(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !g.Enabled() || g.allowed(r) {
			next.ServeHTTP(w, r)
			return
		}
		w.Header().Set("Cache-Control", "no-store")
		if wantsHTML(r) {
			// 浏览器导航：给密码页（200，自包含无外部依赖，不受门禁影响可正常渲染）。
			w.Header().Set("Content-Type", "text/html; charset=utf-8")
			w.WriteHeader(http.StatusOK)
			_, _ = w.Write([]byte(gateHTML))
			return
		}
		// API / 资源请求：401，由调用方（payload/脚本）自行处理。
		http.Error(w, "unauthorized: password required", http.StatusUnauthorized)
	})
}

// allowed 判断请求是否放行：豁免路径或持有效会话 Cookie。
func (g *Gate) allowed(r *http.Request) bool {
	if r.URL.Path == AuthPath {
		return true
	}
	c, err := r.Cookie(CookieName)
	if err != nil || c.Value == "" {
		return false
	}
	return g.verifyToken(c.Value)
}

// wantsHTML 浏览器导航请求（Accept 带 text/html）才回密码页；fetch/XHR/视频流回 401。
func wantsHTML(r *http.Request) bool {
	return strings.Contains(r.Header.Get("Accept"), "text/html")
}

// API 是密码验证接口：GET 查状态 / POST 验密码发会话。
func (g *Gate) API() http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		switch r.Method {
		case http.MethodGet:
			writeJSON(w, http.StatusOK, map[string]any{
				"enabled": g.Enabled(),
				"authed":  g.allowed(r),
			})
		case http.MethodPost:
			ip, _, _ := net.SplitHostPort(r.RemoteAddr)
			if ip == "" {
				ip = r.RemoteAddr
			}
			if !g.acquire(ip) {
				writeJSON(w, http.StatusTooManyRequests, map[string]any{
					"ok": false, "error": "失败次数过多，请一分钟后再试",
				})
				return
			}
			var body struct {
				Password string `json:"password"`
			}
			if err := json.NewDecoder(io.LimitReader(r.Body, 4*1024)).Decode(&body); err != nil {
				writeJSON(w, http.StatusBadRequest, map[string]any{"ok": false, "error": "bad json"})
				return
			}
			stored, _ := g.cfg.GetSetting("securityPassword")
			if stored == "" || subtle.ConstantTimeCompare([]byte(body.Password), []byte(stored)) != 1 {
				n := g.fail(ip)
				log.Printf("[security] 密码验证失败 (%s) 连续第 %d 次", ip, n)
				writeJSON(w, http.StatusUnauthorized, map[string]any{"ok": false, "error": "密码不正确"})
				return
			}
			g.reset(ip)
			maxAge := sessionDays * 24 * 3600
			http.SetCookie(w, &http.Cookie{
				Name:     CookieName,
				Value:    g.sign(time.Now().AddDate(0, 0, sessionDays).Unix()),
				Path:     "/",
				MaxAge:   maxAge,
				HttpOnly: true,
				SameSite: http.SameSiteLaxMode,
			})
			log.Printf("[security] 密码验证通过 (%s)，会话 %d 天", ip, sessionDays)
			writeJSON(w, http.StatusOK, map[string]any{"ok": true})
		default:
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		}
	}
}

// acquire 防爆破闸：锁定中的来源直接拒绝；返回是否放行本次尝试。
func (g *Gate) acquire(ip string) bool {
	g.mu.Lock()
	defer g.mu.Unlock()
	g.pruneLocked()
	ft, ok := g.fails[ip]
	if !ok {
		return true
	}
	return time.Now().After(ft.lockedUntil)
}

// fail 记一次失败；达到阈值则锁定 lockout 时长。返回累计失败次数。
func (g *Gate) fail(ip string) int {
	g.mu.Lock()
	defer g.mu.Unlock()
	ft := g.fails[ip]
	if ft == nil {
		ft = &failTrack{}
		g.fails[ip] = ft
	}
	ft.count++
	ft.lastFail = time.Now()
	if ft.count >= maxFails {
		ft.lockedUntil = time.Now().Add(lockout)
		ft.count = 0 // 锁定窗口重新计
	}
	return ft.count
}

// reset 验证成功清空该来源的失败记录。
func (g *Gate) reset(ip string) {
	g.mu.Lock()
	defer g.mu.Unlock()
	delete(g.fails, ip)
}

// pruneLocked 清掉长期不活跃的失败记录（10 分钟无失败即忘）。
func (g *Gate) pruneLocked() {
	cut := time.Now().Add(-10 * time.Minute)
	for ip, ft := range g.fails {
		if ft.lastFail.Before(cut) {
			delete(g.fails, ip)
		}
	}
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

// gateHTML 是密码页（自包含单文件，样式与上游错误页/管理页同一语言）。
const gateHTML = `<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>AniBox · 访问验证</title>
<style>
  :root { color-scheme: light dark; }
  * { box-sizing: border-box; }
  body { font-family: system-ui, -apple-system, "PingFang SC", sans-serif; min-height: 100vh;
         display: flex; align-items: center; justify-content: center; margin: 0; padding: 0 16px; }
  .card { width: 340px; max-width: 100%; border: 1px solid #8884; border-radius: 14px; padding: 28px 26px; }
  h1 { font-size: 19px; margin: 0 0 4px; text-align: center; }
  .sub { color: #888; font-size: 12.5px; text-align: center; margin-bottom: 18px; }
  input { width: 100%; padding: 9px 11px; border-radius: 9px; border: 1px solid #8886;
          background: transparent; color: inherit; font-size: 14px; }
  button { width: 100%; margin-top: 10px; padding: 9px 0; border-radius: 9px; border: none;
           cursor: pointer; font-size: 13.5px; font-weight: 600; color: #fff; background: #2a89a8; }
  .err { color: #c44; font-size: 12.5px; min-height: 16px; margin-top: 10px; text-align: center; }
</style>
</head>
<body>
  <div class="card">
    <h1>AniBox</h1>
    <div class="sub">本服务已开启访问密码，请验证后继续</div>
    <form id="f">
      <input id="pw" type="password" autocomplete="current-password" placeholder="访问密码" autofocus>
      <button type="submit" id="go">确认</button>
    </form>
    <div class="err" id="err"></div>
  </div>
  <script>
    const f = document.getElementById('f');
    f.addEventListener('submit', async (e) => {
      e.preventDefault();
      const btn = document.getElementById('go');
      const err = document.getElementById('err');
      btn.disabled = true; btn.textContent = '验证中…'; err.textContent = '';
      try {
        const r = await fetch('/app/fntvplus/api/auth', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ password: document.getElementById('pw').value })
        });
        if (r.ok) { location.reload(); return; }
        const j = await r.json().catch(() => ({}));
        err.textContent = j.error || ('验证失败 (' + r.status + ')');
      } catch (ex) { err.textContent = '网络错误：' + ex; }
      btn.disabled = false; btn.textContent = '确认';
    });
  </script>
</body>
</html>`
