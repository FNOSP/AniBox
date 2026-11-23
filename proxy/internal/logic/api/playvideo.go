package api

import (
	"context"
	"crypto/tls"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"regexp"
	"proxy/pkg/fnapi"
	"proxy/pkg/logger"
	"proxy/pkg/utils"
	"strings"
	"time"

	"github.com/gin-gonic/gin"
	"golang.org/x/time/rate"
)

// fetchPlayInfo [lc-904] 带自愈合重试地获取播放信息。并发/竞态下单个 fnOS 请求可能瞬态失败(超时或业务错误),
// 直接 500 会被 node-mpv 捕获为"加载播放列表失败"。这里最多重试 3 次, 仅首次走缓存, 后续绕过缓存避免命中瞬时空/错结果。
func fetchPlayInfo(fnApi *fnapi.ApiService, itemGuid string) (*fnapi.ApiResponse[fnapi.StreamListResponse], error) {
	var lastErr error
	for attempt := 0; attempt < 3; attempt++ {
		var resp *fnapi.ApiResponse[fnapi.StreamListResponse]
		var err error
		if attempt == 0 {
			resp, err = fnApi.GetStreamListCached(itemGuid)
		} else {
			resp, err = fnApi.GetStreamList(itemGuid)
		}
		if err == nil && resp != nil && resp.Success && len(resp.Data.VideoStreams) > 0 {
			return resp, nil
		}
		lastErr = err
		if attempt < 2 {
			time.Sleep(300 * time.Millisecond)
		}
	}
	return nil, lastErr
}

// fetchStream [lc-904] 同上, 带自愈合重试地获取视频流地址。
func fetchStream(fnApi *fnapi.ApiService, mediaGuid, account string) (*fnapi.ApiResponse[fnapi.StreamResponse], error) {
	var lastErr error
	for attempt := 0; attempt < 3; attempt++ {
		var resp *fnapi.ApiResponse[fnapi.StreamResponse]
		var err error
		if attempt == 0 {
			resp, err = fnApi.GetStreamCached(mediaGuid, account)
		} else {
			resp, err = fnApi.GetStream(mediaGuid, utils.StringToUUID(account))
		}
		if err == nil && resp != nil && resp.Success {
			return resp, nil
		}
		lastErr = err
		if attempt < 2 {
			time.Sleep(300 * time.Millisecond)
		}
	}
	return nil, lastErr
}

var (
	// 115盘请求速率限制器，1s/次
	globalLimiter = rate.NewLimiter(rate.Limit(1), 1)
	globalCtx     = context.Background()
)

func waitLimiter() error {
	return globalLimiter.Wait(globalCtx)
}

// parseQueryParam 解析查询参数
func parseQueryParam(c *gin.Context) (*PlayVideoParams, error) {
	var params PlayVideoParams

	// 先绑定 URL Path 里的参数
	if err := c.ShouldBindUri(&params); err != nil {
		return nil, err
	}

	// 再绑定 query 参数
	if err := c.ShouldBindQuery(&params); err != nil {
		return nil, err
	}

	if params.Domain == "" || params.Token == "" || params.ItemGuid == "" || params.Account == "" {
		return nil, errors.New("missing required parameters")
	}

	return &params, nil
}

// ParseCloudInfo 解析云存储信息
func ParseCloudInfo(info fnapi.StreamResponse) *CloudStorageInfo {
	if info.CloudStorageInfo == nil {
		return nil
	}

	// 没有直链
	if len(info.DirectLinkQualities) <= 0 {
		return nil
	}

	result := &CloudStorageInfo{}
	result.DownloadURL = info.DirectLinkQualities[0].URL

	if len(info.Header.Cookie) > 0 {
		result.Cookie = strings.Join(info.Header.Cookie, "; ")
	}

	result.CloudType = CloudType(info.CloudStorageInfo.CloudStorageType)

	return result
}

func getStreamUserAgent(info fnapi.StreamResponse) string {
	for _, userAgent := range info.Header.UserAgent {
		userAgent = strings.TrimSpace(userAgent)
		if userAgent != "" {
			return userAgent
		}
	}

	return ""
}

// briefURL 取 host+path（丢掉体积巨大的签名 query），用于 Info 级日志
func briefURL(raw string) string {
	u, err := url.Parse(raw)
	if err != nil || u.Host == "" {
		if len(raw) > 96 {
			return raw[:96] + "…"
		}
		return raw
	}
	return u.Host + u.Path
}

func PlayVideoHandler(c *gin.Context) {
	params, err := parseQueryParam(c)
	if err != nil {
		logger.Errorf("解析参数失败: %v", err)
		c.JSON(400, gin.H{"error": "Invalid parameters"})
		return
	}

	fnApi := fnapi.NewApiService(params.Domain, params.Token, params.SkipVerify == 1, params.Cookie)
	resp, err := fetchPlayInfo(fnApi, params.ItemGuid)
	if err != nil || resp == nil || !resp.Success || len(resp.Data.VideoStreams) == 0 {
		logger.Errorf("获取播放信息失败或为空: %v", err)
		c.JSON(500, gin.H{"error": "Failed to get play info"})
		return
	}

	// 选择视频流
	videoStreams := resp.Data.VideoStreams
	targetMediaGuid := videoStreams[0].MediaGUID
	if params.SourceIndex > 0 && int(params.SourceIndex) < len(videoStreams) {
		targetMediaGuid = videoStreams[params.SourceIndex].MediaGUID
	}

	// 获取流地址信息
	// [lc-1004] 先探测直链缓存是否命中：115 的限速只应作用于「真的要去 fnOS 重新解析直链」，
	// 而不是播放器的每个 Range/续传/拖动请求（原实现让 115 源每次拖动都白等 1s）。
	// 同理，若起播预热已经在解析同一条直链，本请求只是搭便车等结果，也不该再排一次队。
	freshResolve := !fnApi.HasStreamCached(targetMediaGuid, params.Account) &&
		!fnapi.IsStreamInflight(targetMediaGuid, params.Account)
	streamResp, err := fetchStream(fnApi, targetMediaGuid, params.Account)
	if err != nil || streamResp == nil || !streamResp.Success {
		logger.Errorf("获取视频流失败: %v", err)
		c.JSON(500, gin.H{"error": "Failed to get stream"})
		return
	}

	// [lc-2304] 诊断打点：飞牛返回的 Qualities 到底有几档、每档什么参数。
	// 结论用途：判断「普通剧集清晰度切换」是否可行 ——
	//   · Qualities 只给一档 → 该片源单版本，切换无意义；
	//   · 多档 → 可尝试按 level 取不同转码流。
	// 只在**首次解析**（非缓存命中）时打，避免拖动/续传把日志刷满。
	if freshResolve {
		if qs := streamResp.Data.Qualities; len(qs) > 0 {
			parts := make([]string, 0, len(qs))
			for _, q := range qs {
				parts = append(parts, fmt.Sprintf("%s/%dkbps/prog=%v/m3u8=%v",
					q.Resolution, q.Bitrate, q.Progressive, q.IsM3U8))
			}
			logger.Infof("[lc-2304] 飞牛 qualities(%d 档): %s | 媒体 %dx%d",
				len(qs), strings.Join(parts, " ; "),
				streamResp.Data.VideoStream.Width, streamResp.Data.VideoStream.Height)
		} else {
			logger.Infof("[lc-2304] 飞牛 qualities: 空（该片源单版本）")
		}
		if dl := streamResp.Data.DirectLinkQualities; len(dl) > 0 {
			parts := make([]string, 0, len(dl))
			for _, q := range dl {
				parts = append(parts, fmt.Sprintf("%s/%dkbps", q.Resolution, q.Bitrate))
			}
			logger.Infof("[lc-2304] 直链档位(%d): %s", len(dl), strings.Join(parts, " ; "))
		}
	}

	var (
		targetUrl    = fnApi.GetVideoURL(targetMediaGuid)
		proxyType    = TransparentProxy
		skipVerify   = params.SkipVerify == 1
		extraHeaders = utils.PassthroughHeaders(c.Request)
	)

	cloudInfo := ParseCloudInfo(streamResp.Data)
	streamUserAgent := getStreamUserAgent(streamResp.Data)
	useCloudDirect := cloudInfo != nil && params.UseNasLocal != 1

	// 云盘直链模式
	if useCloudDirect {
		logger.Infof("启用云存储直连: type=%d", cloudInfo.CloudType)

		targetUrl = cloudInfo.DownloadURL
		// 禁止跳过证书验证，云厂商的证书通常是合法的，不需要跳过验证
		skipVerify = false

		// 注入云盘需要的 Cookie
		if cloudInfo.Cookie != "" {
			extraHeaders["Cookie"] = cloudInfo.Cookie
		}

		if streamUserAgent != "" {
			extraHeaders["User-Agent"] = streamUserAgent
		} else {
			// [lc-1003] 云盘未返回 UA 时，Go 默认发 "Go-http-client/1.1"，百度/115 直链常对此类
			// 非浏览器 UA 限速或降权（即便 SVIP 也可能被按客户端类型节流）。补一个浏览器 UA，
			// 确保拿到满速。仅当 fnOS 未下发 UA 时才生效，不会覆盖其下发的签名 UA。
			extraHeaders["User-Agent"] = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
		}

		// 选择播放策略
		switch cloudInfo.CloudType {
		case QuarkPan:
			proxyType = ChunkedProxy // 夸克需要切片
		case Cloud115Pan:
			// 115 特殊处理：UA 和限流
			// [lc-1004] 仅在真的重新解析直链时限速。命中缓存说明这是播放器的续传/拖动请求，
			// 排队 1s 只会让拖动变卡，对风控没有任何意义（并没有产生新的解析请求）。
			if freshResolve {
				_ = waitLimiter()
			}
			// 其他网盘使用默认的 TransparentProxy
		}
	} else {
		// 本地 NAS 转发模式 ---
		// 只有请求 NAS 时才需要 Authorization Token
		extraHeaders["Authorization"] = params.Token
		// [lc-295] 注入 persist:fntv 会话 Cookie(Trim-MC-token), 与 webview/主进程同源鉴权,
		// 否则 NAS 媒体流接口会被弹回登录页 HTML(playvideo 返回 500 / 解析 JSON 失败 '<')。
		extraHeaders["Cookie"] = params.Cookie + "; mode=relay"
	}

	// 执行代理
	// [lc-1004] 云盘直链的签名 query 常有数百字节，每请求全量打印是撑爆 stdout 管道的主因之一。
	// Info 只留 host+path（足以判断打到了哪台 CDN），完整 URL 降到 Debug。
	logger.Infof("开始代理 | 模式: %v | 目标: %s", proxyType, briefURL(targetUrl))
	logger.Debugf("开始代理 | 完整目标 URL: %s", targetUrl)

	switch proxyType {
	case ChunkedProxy:
		// 边下边播处理 (如夸克)
		handler := utils.NewCloudStorageHandler(targetUrl, extraHeaders, skipVerify)
		handler.HandleRequest(c)
	default:
		// 透明代理 (本地 NAS 或 115/阿里等直链)
		utils.DynamicProxy(c, targetUrl, extraHeaders, skipVerify)
	}
}

// QualitiesHandler [lc-2304] 返回该条目可选的清晰度档位。
//
// 用途：应用在起播前查一次，判断「这部剧有没有多档清晰度」——
// 有则把档位推给 mpv 底栏的清晰度按钮；只有一档就不显示按钮（不做假档位）。
//
// 数据来源：飞牛 `stream/list` 的元数据里带 `qualities[]`
// （resolution / bitrate / progressive / is_m3u8），不是额外请求。
//
// 注意：飞牛**本地播放走 `/media/range/<mediaGUID>`（原始文件直读）**，
// qualities 只描述该片源的可能档位。真正按档位取流需要走转码通道，
// 本接口只负责如实回传元数据，不承诺"切换能改变实际码流"。
func QualitiesHandler(c *gin.Context) {
	var params PlayVideoParams
	if err := c.ShouldBindUri(&params); err != nil {
		c.JSON(400, gin.H{"error": "Invalid parameters"})
		return
	}
	if err := c.ShouldBindQuery(&params); err != nil {
		c.JSON(400, gin.H{"error": "Invalid parameters"})
		return
	}
	if params.Domain == "" || params.Token == "" || params.ItemGuid == "" {
		c.JSON(400, gin.H{"error": "missing required parameters"})
		return
	}

	fnApi := fnapi.NewApiService(params.Domain, params.Token, params.SkipVerify == 1, params.Cookie)
	resp, err := fetchPlayInfo(fnApi, params.ItemGuid)
	if err != nil || resp == nil || !resp.Success || len(resp.Data.VideoStreams) == 0 {
		logger.Errorf("qualities: 获取播放信息失败: %v", err)
		c.JSON(500, gin.H{"error": "Failed to get play info"})
		return
	}

	// 按 SourceIndex 选版本（与 playvideo 同口径），再取该媒体的 qualities
	videoStreams := resp.Data.VideoStreams
	mediaGuid := videoStreams[0].MediaGUID
	if params.SourceIndex > 0 && int(params.SourceIndex) < len(videoStreams) {
		mediaGuid = videoStreams[params.SourceIndex].MediaGUID
	}

	streamResp, err := fetchStream(fnApi, mediaGuid, params.Account)
	if err != nil || streamResp == nil || !streamResp.Success {
		logger.Errorf("qualities: 获取流信息失败: %v", err)
		c.JSON(500, gin.H{"error": "Failed to get stream"})
		return
	}

	type item struct {
		Resolution  string `json:"resolution"`
		Bitrate     int    `json:"bitrate"`
		Progressive bool   `json:"progressive"`
		IsM3U8      bool   `json:"isM3U8"`
	}
	items := make([]item, 0, len(streamResp.Data.Qualities))
	for _, q := range streamResp.Data.Qualities {
		items = append(items, item{
			Resolution:  q.Resolution,
			Bitrate:     q.Bitrate,
			Progressive: q.Progressive,
			IsM3U8:      q.IsM3U8,
		})
	}
	// 同时把片源本身的宽高带上：应用据此判断"这一档是不是等于源分辨率"
	// [lc-2307] **fileGuid 是关键**：`play/play` 的 media_guid 要的是
	// `FileStream.guid`（fv_ 开头），不是 VideoStream.MediaGUID。
	// 飞鲸 createPlayRequest 写的是 `mediaGuid: fileStream.guid`
	// （player_session_coordinator.dart:793）。传错会得到
	// 「System busy, please retry later」。
	// [lc-2309] 一并回传 videoGuid / audioGuid：飞鲸的 play/play 必传这两个
	// （createPlayRequest 从 videoStream.guid / 选中音轨拿），传空很可能就是
	// "System busy" 的另一半原因。audio 取 is_default=1 的那条，没有就取第一条。
	audioGuid := ""
	for _, a := range streamResp.Data.AudioStreams {
		if a.IsDefault == 1 {
			audioGuid = a.GUID
			break
		}
	}
	if audioGuid == "" && len(streamResp.Data.AudioStreams) > 0 {
		audioGuid = streamResp.Data.AudioStreams[0].GUID
	}
	c.JSON(200, gin.H{
		"mediaGuid": mediaGuid,
		"fileGuid":  streamResp.Data.FileStream.GUID,
		"videoGuid": streamResp.Data.VideoStream.GUID,
		// [lc-2309] 源编码：调用方据此按飞鲸规则选 video_encoder
		//（hevc/av1/vp8/vp9 → hevc，其余 → h264；forced h264 优先）
		"codecName": streamResp.Data.VideoStream.CodecName,
		"profile":   streamResp.Data.VideoStream.Profile,
		"audioGuid": audioGuid,
		"width":     streamResp.Data.VideoStream.Width,
		"height":    streamResp.Data.VideoStream.Height,
		"qualities": items,
	})
}

// TranscodePlayHandler [lc-2305] 请求指定清晰度的**转码播放地址**。
//
// 照抄 FlyNarwhal 的 switchURL 流程（player_screen.dart:3901 / 3967）：
//   1. 先 quit 旧的转码会话（可选，由调用方传 oldPlayLink）；
//   2. 带新的 resolution/bitrate 调 play/play；
//   3. 返回 play_link，由客户端重新 loadfile。
//
// **只有选非原画档才应调用**：原画档继续走 /api/v1/playvideo（原始文件直读），
// 这样不触发 NAS 转码。这是飞鲸的 supportsDirectLink 口径：
//   quality.resolution == qualities[0].resolution && quality.bitrate == qualities[0].bitrate
// 相等才算原画 → 直链；否则必须转码。
func TranscodePlayHandler(c *gin.Context) {
	var body struct {
		Domain       string `json:"domain"`
		Token        string `json:"token"`
		Cookie       string `json:"cookie"`
		SkipVerify   int32  `json:"skipVerify"`
		ItemGuid     string `json:"itemGuid"`
		MediaGuid    string `json:"mediaGuid"`
		FileGuid     string `json:"fileGuid"` // [lc-2307] FileStream.guid —— play/play 真正要的
		VideoGuid    string `json:"videoGuid"`
		VideoEncoder string `json:"videoEncoder"`
		Resolution   string `json:"resolution"`
		Bitrate      int    `json:"bitrate"`
		AudioGuid    string `json:"audioGuid"`
		SubtitleGuid string `json:"subtitleGuid"`
		StartSeconds int    `json:"startSeconds"`
		ForcedSDR    int    `json:"forcedSdr"`
		OldPlayLink  string `json:"oldPlayLink"`
	}
	if err := c.ShouldBindJSON(&body); err != nil {
		c.JSON(400, gin.H{"error": "Invalid parameters"})
		return
	}
	if body.Domain == "" || body.Token == "" {
		c.JSON(400, gin.H{"error": "missing required parameters"})
		return
	}

	fnApi := fnapi.NewApiService(body.Domain, body.Token, body.SkipVerify == 1, body.Cookie)

	// ① [lc-2307] 主路径：**原地换档**（照抄 FlyNarwhal player_screen.dart:4738）。
	// 飞鲸切换清晰度首选 media/p 的 req=media.resetQuality —— 服务端在**同一个
	// 转码会话**里换编码参数，返回 updateM3u8/hlsTime，客户端继续播原 m3u8，
	// **不需要** quit + play/play 重建（那条路只在「直链 → 转码」首建时用）。
	// 只有没有旧会话（OldPlayLink 为空）时才走下面的 play/play 新建。
	if body.OldPlayLink != "" {
		reqID := fmt.Sprintf("anibox-%d", time.Now().UnixNano())
		resp, err := fnApi.ResetQuality(body.OldPlayLink, body.Resolution,
			body.Bitrate, body.StartSeconds, reqID)
		if err == nil && resp != nil && resp.Success {
			logger.Infof("[lc-2307] 原地换档成功: %s @ %dkbps（复用会话 %s）",
				body.Resolution, body.Bitrate/1000, briefURL(body.OldPlayLink))
			c.JSON(200, gin.H{
				"playLink": body.OldPlayLink, // 仍是同一个 m3u8 地址
				"inPlace":  true,
			})
			return
		}
		// 原地换档失败 → 退化为重建会话（下面走 play/play）
		logger.Warnf("[lc-2307] 原地换档失败，回退到重建会话: err=%v", err)
	}

	// ② 目标编码器归一：只允许 h264 / hevc。传源码编码（如 av1）会被服务端
	//    以 ParameterError 8192 拒绝（FlyNarwhal createPlayRequest 的注释说明）。
	enc := strings.ToLower(strings.TrimSpace(body.VideoEncoder))
	if enc != "h264" && enc != "hevc" {
		enc = "h264"
	}

	// [lc-2307] media_guid 必须是 FileStream.guid（飞鲸同口径）。
	// 没拿到 fileGuid 时才退回 mediaGuid（老调用方兼容）。
	playMediaGuid := body.FileGuid
	if playMediaGuid == "" {
		playMediaGuid = body.MediaGuid
	}
	req := fnapi.PlayPlayRequestData{
		MediaGUID:      playMediaGuid,
		VideoGUID:      body.VideoGuid,
		VideoEncoder:   enc,
		Resolution:     body.Resolution,
		Bitrate:        body.Bitrate,
		StartTimestamp: body.StartSeconds,
		AudioEncoder:   "aac",
		AudioGUID:      body.AudioGuid,
		SubtitleGUID:   body.SubtitleGuid,
		Channels:       2,
		ForcedSDR:      body.ForcedSDR,
	}
	resp, err := fnApi.PlayPlay(req)
	if err != nil || resp == nil || !resp.Success || resp.Data.PlayLink == "" {
		msg := ""
		if resp != nil {
			msg = resp.Message
		}
		logger.Errorf("[lc-2305] play/play 失败: err=%v msg=%s", err, msg)
		c.JSON(500, gin.H{"error": "Failed to create transcode session", "detail": msg})
		return
	}

	// ③ [lc-2312] **不能把裸 NAS 地址交给 mpv**：preset.m3u8 要求 Authx+Cookie，
	// 直连必被拒（实测 TCP 通但 end-file 无内容）。统一包成代理地址：
	//   /api/v1/hls/v/media/<id>/preset.m3u8?token=...&domain=...&cookie=...
	// 这样 mpv 只连 127.0.0.1，凭据由代理注入。
	raw := resp.Data.PlayLink
	if strings.HasPrefix(raw, "/") {
		raw = strings.TrimSuffix(raw, "/")
	} else {
		// 绝对地址：剥掉 domain 前缀，只留相对路径给 hls 路由
		raw = strings.TrimPrefix(raw, strings.TrimRight(body.Domain, "/"))
	}
	// 代理自身地址：从请求 Host 推导（避免硬编码端口）
	selfBase := "http://" + c.Request.Host
	link := fmt.Sprintf("%s/api/v1/hls%s?token=%s&skipVerify=%d&domain=%s&cookie=%s",
		selfBase,
		strings.TrimRight(raw, "/"),
		url.QueryEscape(body.Token),
		body.SkipVerify,
		url.QueryEscape(strings.TrimRight(body.Domain, "/")),
		url.QueryEscape(body.Cookie),
	)
	logger.Infof("[lc-2305] 转码会话已建: %s @ %dkbps (%s) start=%ds",
		body.Resolution, body.Bitrate/1000, enc, body.StartSeconds)
	c.JSON(200, gin.H{"playLink": link})
}

// HlsProxyHandler [lc-2312] 转发飞牛 HLS 转码流（preset.m3u8 及其分片）。
//
// 为什么必须经过代理：转码会话返回的 play_link 是**飞牛原始地址**
// （`http://<nas>:17777/v/media/<id>/preset.m3u8`），该路径要求 Authx 签名 +
// Cookie；mpv 直接访问会被拒（实测：TCP 连上但拿不到内容 → end-file）。
// 而普通直链走的是 `/api/v1/playvideo/<guid>`，由代理带凭据转发，所以一直正常。
//
// 路由形态：GET /api/v1/hls/*upstream
//   upstream = 飞牛侧相对路径（如 `v/media/<id>/preset.m3u8`）
//   查询参数里带 token/domain/cookie/skipVerify（与 playvideo 同一套）
func HlsProxyHandler(c *gin.Context) {
	upstream := c.Param("upstream")
	if upstream == "" {
		c.JSON(400, gin.H{"error": "missing upstream path"})
		return
	}
	var q struct {
		Token      string `form:"token"`
		Domain     string `form:"domain"`
		Cookie     string `form:"cookie"`
		SkipVerify int32  `form:"skipVerify"`
		Sid        string `form:"sid"` // [lc-2315] 会话码：有它就不必带长凭据
	}
	if err := c.ShouldBindQuery(&q); err != nil {
		c.JSON(400, gin.H{"error": "missing required parameters"})
		return
	}
	// [lc-2315] 分片请求只带 sid；m3u8 首请求带完整凭据
	if q.Sid != "" {
		if cred, ok := getHlsSession(q.Sid); ok {
			q.Token, q.Domain, q.Cookie, q.SkipVerify =
				cred.token, cred.domain, cred.cookie, cred.skipVerify
		} else {
			c.JSON(410, gin.H{"error": "hls session expired"})
			return
		}
	}
	if q.Domain == "" {
		c.JSON(400, gin.H{"error": "missing required parameters"})
		return
	}

	// 拼回飞牛原始 URL（保留分片/子播放列表的查询串）
	target := strings.TrimRight(q.Domain, "/") + "/" + strings.TrimLeft(upstream, "/")
	if raw := c.Request.URL.RawQuery; raw != "" {
		// 把代理自己的参数剔掉，剩下的（分片签名等）原样带上
		kept := make([]string, 0, 4)
		for _, kv := range strings.Split(raw, "&") {
			k := kv
			if i := strings.IndexByte(kv, '='); i >= 0 {
				k = kv[:i]
			}
			switch k {
			case "token", "domain", "cookie", "skipVerify":
				continue
			}
			kept = append(kept, kv)
		}
		if len(kept) > 0 {
			target += "?" + strings.Join(kept, "&")
		}
	}

	// [lc-2312] m3u8 必须**自己拉+重写**，不能用 DynamicProxy：
	// 它会把分片重写成「飞牛绝对地址」，mpv 直连分片一样会被拒
	// （分片同样要求 Authx/Cookie）。这里把所有 URI 重写成**再代理**的形式：
	//   /api/v1/hls/v/media/<id>/seg-1.ts?token=...&domain=...&cookie=...
	isPlaylist := strings.HasSuffix(strings.ToLower(strings.SplitN(upstream, "?", 2)[0]), ".m3u8")
	if isPlaylist {
		req, err := http.NewRequest("GET", target, nil)
		if err != nil {
			c.JSON(500, gin.H{"error": "bad upstream url"})
			return
		}
		req.Header.Set("Authorization", q.Token)
		req.Header.Set("Cookie", q.Cookie+"; mode=relay")
		client := &http.Client{
			Timeout: 20 * time.Second,
			Transport: &http.Transport{
				TLSClientConfig: &tls.Config{InsecureSkipVerify: q.SkipVerify == 1},
			},
		}
		resp, err := client.Do(req)
		if err != nil {
			logger.Errorf("[hls] 拉取播放列表失败: %v", err)
			c.JSON(502, gin.H{"error": "upstream unreachable", "detail": err.Error()})
			return
		}
		defer resp.Body.Close()
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 8<<20))
		if resp.StatusCode != 200 {
			logger.Errorf("[hls] 上游返回 %d", resp.StatusCode)
			c.Data(resp.StatusCode, "text/plain", body)
			return
		}

		// 代理自身的 hls 前缀（重写 base 用它，而不是飞牛地址）
		selfBase := "http://" + c.Request.Host + "/api/v1/hls/"
		// [lc-2315] 分片 URL 只带**会话码**，不再拼长凭据串：
		// URL 从 ~400 字节降到 ~80 字节，HLS 分片拉取与 yt-dlp 探测都快得多。
		sid := newHlsSession(hlsCred{
			token: q.Token, domain: strings.TrimRight(q.Domain, "/"),
			cookie: q.Cookie, skipVerify: q.SkipVerify,
		})
		qp := "sid=" + sid
		// 把上游 m3u8 里的相对/绝对 URI 全部改写成「代理 + 相对上游路径 + 凭据」
		u, _ := url.Parse(target)
		rewritten := rewriteHlsToProxy(string(body), u, selfBase, qp)
		c.Header("Content-Type", "application/vnd.apple.mpegurl")
		c.Header("Cache-Control", "no-store")
		c.String(200, "%s", rewritten)
		return
	}

	// 分片（.ts/.m4s 等）：直接带凭据转发字节流
	headers := map[string]string{
		"Authorization": q.Token,
		"Cookie":        q.Cookie + "; mode=relay",
	}
	logger.Debugf("[hls] 转发分片 %s", briefURL(target))
	utils.DynamicProxy(c, target, headers, q.SkipVerify == 1)
}

// rewriteHlsToProxy [lc-2312] 把 m3u8 里每个 URI 改写成经代理的地址。
//
// 规则：先把 URI 解析成**飞牛的绝对地址**（相对路径按 m3u8 所在目录拼接），
// 再把 scheme+host 换成代理，路径改成 `/api/v1/hls/<相对路径>`，带上凭据查询串。
// 这样 m3u8 与全部分片都只连 127.0.0.1，鉴权由代理注入。
func rewriteHlsToProxy(content string, base *url.URL, selfBase, qp string) string {
	if base == nil {
		return content
	}
	toProxy := func(raw string) string {
		u, err := url.Parse(strings.TrimSpace(raw))
		if err != nil || u.String() == "" {
			return raw
		}
		abs := base.ResolveReference(u)
		// 只保留 path（去掉飞牛的 query，分片签名由我们重新带）
		p := abs.Path
		return selfBase + strings.TrimLeft(p, "/") + "?" + qp
	}
	lines := strings.Split(content, "\n")
	for i, line := range lines {
		t := strings.TrimSpace(line)
		if t == "" {
			continue
		}
		if strings.HasPrefix(t, "#") {
			// EXT-X-KEY / EXT-X-MAP 里的 URI="..."
			if strings.Contains(t, `URI="`) {
				lines[i] = reHlsURI.ReplaceAllStringFunc(t, func(m string) string {
					inner := m[len(`URI="`):]
					inner = strings.TrimSuffix(inner, `"`)
					return `URI="` + toProxy(inner) + `"`
				})
			}
			continue
		}
		lines[i] = toProxy(t)
	}
	return strings.Join(lines, "\n")
}

var reHlsURI = regexp.MustCompile(`URI="[^"]*"`)
