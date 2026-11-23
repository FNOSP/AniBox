package api

import (
	"context"
	"errors"
	"fmt"
	"net/url"
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
	c.JSON(200, gin.H{
		"mediaGuid": mediaGuid,
		"width":     streamResp.Data.VideoStream.Width,
		"height":    streamResp.Data.VideoStream.Height,
		"qualities": items,
	})
}
