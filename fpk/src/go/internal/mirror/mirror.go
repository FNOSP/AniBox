// Package mirror —— AniBox 自建镜像（TMDB / Bangumi）的准入签名与请求回退。
//
// 协议与桌面端 src-tauri/src/mirror_sign.rs **逐字节一致**（worker verify()）：
//
//	x-fntv-app : fntv-plus
//	x-fntv-ts  : 当前 Unix 秒（服务端 ±300s 时间窗）
//	x-fntv-sign: base64( sha256( "fntv-plus" + ts + METHOD + pathWithQuery + hex(sha256(body)) + secret ) )
//
// ⚠ 内层 body 摘要是 **hex**，外层整体才是 **base64**（写反 = bad_signature）。
// pathWithQuery 含前导斜杠与 query 原文，例：`/v0/subjects/8`、
// `/3/search/tv?query=x&language=zh-CN`。
//
// 密钥来源：运行时环境变量 → 编译期 `-ldflags -X` 注入的包级变量 → 空（视为镜像不可用）。
// 仓库里**不写任何密钥**。
package mirror

import (
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"strconv"
	"strings"
	"time"
)

// AppID 两个镜像共用的应用标识（worker verify 的第一道闸）。
const AppID = "fntv-plus"

// 编译期注入变量（-X fntvplus/internal/mirror.TMDBSecret=... / BGMSecret=...）。
// 缺失时为空串，运行期回退直连。
var (
	TMDBSecret = ""
	BGMSecret  = ""
)

// 默认端点（顺序即回退顺序）。
const (
	DefaultTMDBEndpoints = "https://tmdb.690075.xyz,https://fntv-tmdb-mirror.122983191.workers.dev"
	DefaultBGMEndpoints  = "https://bgm.690075.xyz,https://fntv-bgm-mirror.122983191.workers.dev"

	// TMDB 图片镜像（免签名，反代 image.tmdb.org）；对应路径 /img/<rest>。
	TMDBImageBase = "https://tmdb.690075.xyz"
	// Bangumi 图片镜像（签名轨，与 API 同密钥）；对应路径 /img/<rest>。
	BGMImageBase = "https://bgm.690075.xyz"
	// Bangumi 图片直连兜底（无密钥时；客户端侧可能被 DNS 污染，仅作退路）。
	BGMImageDirect = "https://lain.bgm.tv"
)

// NowSecs 当前 Unix 秒。
func NowSecs() int64 { return time.Now().Unix() }

// Sign 计算准入签名：
//
//	bodyDigest = hex(sha256(body))              ← 内层 hex
//	sign       = base64(sha256(appId+ts+method+pathWithQuery+bodyDigest+secret))
func Sign(secret string, ts int64, method, pathWithQuery, body string) string {
	inner := sha256.Sum256([]byte(body))
	payload := AppID + strconv.FormatInt(ts, 10) + method + pathWithQuery +
		hex.EncodeToString(inner[:]) + secret
	outer := sha256.Sum256([]byte(payload))
	return base64.StdEncoding.EncodeToString(outer[:])
}

// ApplyAuth 给请求挂上准入三件套（ts/签名现算，二者用同一个 ts）。
// pathWithQuery 必须与最终请求 URL 的 path+query 完全一致（含顺序）。
func ApplyAuth(req *http.Request, secret, method, pathWithQuery, body string) {
	ts := NowSecs()
	req.Header.Set("x-fntv-app", AppID)
	req.Header.Set("x-fntv-ts", strconv.FormatInt(ts, 10))
	req.Header.Set("x-fntv-sign", Sign(secret, ts, method, pathWithQuery, body))
}

// ResolveSecret 密钥解析：运行时环境变量 → 编译期注入值 → 空串。
// 空白视为未配置（环境变量里常见尾随空白）。
func ResolveSecret(envVar, compiled string) string {
	if s := strings.TrimSpace(os.Getenv(envVar)); s != "" {
		return s
	}
	return strings.TrimSpace(compiled)
}

// ResolveEndpoints 端点列表：环境变量（逗号分隔）覆盖 → 默认值；去空白/去尾斜杠/丢空项。
func ResolveEndpoints(envVar, defaultCSV string) []string {
	raw := strings.TrimSpace(os.Getenv(envVar))
	if raw == "" {
		raw = defaultCSV
	}
	var out []string
	for _, s := range strings.Split(raw, ",") {
		s = strings.TrimRight(strings.TrimSpace(s), "/")
		if s != "" {
			out = append(out, s)
		}
	}
	return out
}

// Kind 失败分类。
type Kind int

const (
	// Degradable 不可达 / 超时 / 5xx / 429 —— 可回退直连。
	Degradable Kind = iota
	// Fatal 确定性拒绝（4xx 含 401）—— 不回退，原样报错。
	Fatal
)

// Error 镜像请求失败。
type Error struct {
	Kind   Kind
	Status int
	Msg    string
}

func (e *Error) Error() string { return e.Msg }

// IsDegradable 判断错误是否允许回退直连。未知错误按可降级处理（多为网络层）。
func IsDegradable(err error) bool {
	if err == nil {
		return false
	}
	var me *Error
	if errors.As(err, &me) {
		return me.Kind == Degradable
	}
	return true
}

// Request 一次镜像请求。
type Request struct {
	Secret        string
	Endpoints     []string
	Method        string
	PathWithQuery string
	Body          string
	// Header 额外请求头（如账号请求的 Authorization）。
	Header  map[string]string
	Timeout time.Duration
	Client  *http.Client
	Accept  string
}

// Response 镜像响应。
type Response struct {
	Status int
	Header http.Header
	Body   []byte
}

const maxBodyBytes = 32 << 20

// Do 多端点回退执行一次镜像请求。
//
//   - 2xx            → (resp, nil)
//   - 4xx（含 401）  → (resp, *Error{Kind: Fatal})，调用方可读取状态/正文
//   - 429            → (resp, *Error{Kind: Degradable})
//   - 网络错误/超时/5xx → 试下一个端点；全部失败 → (nil, *Error{Kind: Degradable})
func Do(r Request) (*Response, error) {
	if strings.TrimSpace(r.Secret) == "" {
		return nil, &Error{Kind: Degradable, Msg: "镜像密钥未配置"}
	}
	if len(r.Endpoints) == 0 {
		return nil, &Error{Kind: Degradable, Msg: "镜像端点未配置"}
	}
	method := r.Method
	if method == "" {
		method = http.MethodGet
	}
	accept := r.Accept
	if accept == "" {
		accept = "application/json"
	}
	client := r.Client
	if client == nil {
		timeout := r.Timeout
		if timeout <= 0 {
			timeout = 12 * time.Second
		}
		client = &http.Client{Timeout: timeout}
	}

	var lastErr string
	for _, ep := range r.Endpoints {
		full := ep + r.PathWithQuery
		u, err := url.Parse(full)
		if err != nil {
			lastErr = fmt.Sprintf("%s URL 非法: %v", ep, err)
			continue
		}
		// 参与签名的 path+query 从**解析后的 URL** 取，与实际发出的请求逐字节一致。
		signed := u.EscapedPath()
		if u.RawQuery != "" {
			signed += "?" + u.RawQuery
		}
		var bodyReader io.Reader
		if r.Body != "" {
			bodyReader = strings.NewReader(r.Body)
		}
		req, err := http.NewRequest(method, full, bodyReader)
		if err != nil {
			lastErr = fmt.Sprintf("%s 构造请求失败: %v", ep, err)
			continue
		}
		req.Header.Set("Accept", accept)
		if req.Header.Get("User-Agent") == "" {
			req.Header.Set("User-Agent", "AniBox/0.1 (fntv-plus)")
		}
		if r.Body != "" {
			req.Header.Set("Content-Type", "application/json")
		}
		for k, v := range r.Header {
			req.Header.Set(k, v)
		}
		ApplyAuth(req, r.Secret, method, signed, r.Body)

		resp, err := client.Do(req)
		if err != nil {
			lastErr = fmt.Sprintf("%s 连接失败/超时: %v", ep, err)
			continue
		}
		data, readErr := io.ReadAll(io.LimitReader(resp.Body, maxBodyBytes))
		_ = resp.Body.Close()
		if readErr != nil {
			lastErr = fmt.Sprintf("%s 读取响应体失败: %v", ep, readErr)
			continue
		}
		out := &Response{Status: resp.StatusCode, Header: resp.Header, Body: data}
		switch {
		case resp.StatusCode >= 200 && resp.StatusCode < 300:
			return out, nil
		case resp.StatusCode == http.StatusTooManyRequests:
			return out, &Error{Kind: Degradable, Status: resp.StatusCode,
				Msg: fmt.Sprintf("%s HTTP 429: %s", ep, truncate(string(data), 160))}
		case resp.StatusCode >= 500:
			lastErr = fmt.Sprintf("%s HTTP %d: %s", ep, resp.StatusCode, truncate(string(data), 160))
			continue
		default: // 4xx（含 401）：确定性拒绝，不降级
			return out, &Error{Kind: Fatal, Status: resp.StatusCode,
				Msg: fmt.Sprintf("%s HTTP %d: %s", ep, resp.StatusCode, truncate(string(data), 200))}
		}
	}
	return nil, &Error{Kind: Degradable, Msg: "所有镜像端点均不可达（最后：" + lastErr + "）"}
}

func truncate(s string, n int) string {
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	return string(r[:n]) + "…"
}
