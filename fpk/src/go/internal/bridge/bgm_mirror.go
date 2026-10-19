// bgm_mirror.go —— Bangumi 数据源的内置镜像优先路径（与桌面端 bgm.rs 对齐）。
//
// 公开数据（/calendar、/v0/subjects、/v0/search/subjects 等）默认走自建镜像
// （bgm.690075.xyz，带准入签名，免用户 Token）；账号相关请求把用户的 Bangumi
// Bearer Token **原样透传**给镜像。镜像缺密钥/网络错误/超时/5xx/429 时回退官方直连
// （公开数据免凭证）；401 等 4xx 不降级，原样交给调用方归类。
package bridge

import (
	"encoding/json"
	"net/http"
	"time"

	"fntvplus/internal/mirror"
)

// bgmSecret 内置 Bangumi 镜像准入密钥：环境变量 FNTV_BGM_SECRET → 编译期注入 → 空。
func (b *Bridge) bgmSecret() string {
	return mirror.ResolveSecret("FNTV_BGM_SECRET", mirror.BGMSecret)
}

// bgmEndpoints 镜像端点（FNTV_BGM_ENDPOINT 覆盖，逗号分隔）。
func (b *Bridge) bgmEndpoints() []string {
	return mirror.ResolveEndpoints("FNTV_BGM_ENDPOINT", mirror.DefaultBGMEndpoints)
}

// bgmMirrorReq 走内置镜像（/v0/*、/p1/*、/calendar 等 + 准入签名）。
// token 非空时以 Bearer 原样透传（账号绑定 / 收藏读写）。
func (b *Bridge) bgmMirrorReq(method, pathWithQuery, token, bodyText string) (*mirror.Response, error) {
	var hdr map[string]string
	if token != "" {
		hdr = map[string]string{"Authorization": "Bearer " + token}
	}
	return mirror.Do(mirror.Request{
		Secret:        b.bgmSecret(),
		Endpoints:     b.bgmEndpoints(),
		Method:        method,
		PathWithQuery: pathWithQuery,
		Body:          bodyText,
		Header:        hdr,
		Timeout:       12 * time.Second,
	})
}

// parseJSONMap 宽松解析 JSON 对象（空 body / 非对象 → nil，不报错）。
func parseJSONMap(data []byte) map[string]any {
	var out map[string]any
	if len(data) > 0 {
		_ = json.Unmarshal(data, &out)
	}
	return out
}

// bgmCalendarData 拉取 /calendar 原始字节：镜像优先，镜像不可用回退官方直连。
func (b *Bridge) bgmCalendarData() ([]byte, error) {
	if b.bgmSecret() != "" {
		resp, err := b.bgmMirrorReq(http.MethodGet, "/calendar", "", "")
		if err == nil {
			return resp.Body, nil
		}
		if resp != nil && !mirror.IsDegradable(err) {
			// 4xx：镜像有明确响应，交给上层按数据/状态处理（不降级）。
			return resp.Body, nil
		}
		logf("[bangumi] calendar 镜像不可用，降级直连：%v", err)
	}
	return b.fetchWithTransport("https://api.bgm.tv/calendar", bangumiUAWeb)
}
