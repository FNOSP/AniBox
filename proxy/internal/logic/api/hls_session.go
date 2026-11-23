package api

// [lc-2315] HLS 会话表：把 token/domain/cookie 从**每个分片 URL** 移到进程内映射。
//
// 为什么必须这么做（实测教训）：
//   · 原方案把完整凭据串（约 400 字节）拼进每个分片 URL，而 HLS 每秒要拉
//     一个新分片 → URL 又长又多，ffmpeg 的 hls demuxer 与 modernz 的
//     yt-dlp 探测都被拖慢，表现为**播放卡顿**；
//   · 更糟的是 modernz 会对每个这样的 URL 起一次 yt-dlp 子进程探测大小，
//     长 URL + 子进程 = 明显卡顿；
//   · smart_skip 也从 URL 里提 query（extract_id_and_query），超长凭据串
//     让它解析失败刷 error。
//
// 新方案：m3u8 重写时分片 URL 只带一个短会话码（8 字节 hex），
// 凭据存进程内 map；分片请求按会话码取回凭据再转发。
import (
	"crypto/rand"
	"encoding/hex"
	"sync"
)

type hlsCred struct {
	token      string
	domain     string
	cookie     string
	skipVerify int32
}

var (
	hlsMu   sync.RWMutex
	hlsSess = map[string]hlsCred{}
)

// newHlsSession 生成会话码（8 字节随机 hex，短且不可猜）。
func newHlsSession(c hlsCred) string {
	b := make([]byte, 8)
	_, _ = rand.Read(b)
	id := hex.EncodeToString(b)
	hlsMu.Lock()
	// 简单容量保护：超过 256 条清空（同时只会有一两条活跃）
	if len(hlsSess) > 256 {
		hlsSess = map[string]hlsCred{}
	}
	hlsSess[id] = c
	hlsMu.Unlock()
	return id
}

// getHlsSession 按会话码取凭据。
func getHlsSession(id string) (hlsCred, bool) {
	hlsMu.RLock()
	c, ok := hlsSess[id]
	hlsMu.RUnlock()
	return c, ok
}
