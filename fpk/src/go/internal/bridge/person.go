// Package bridge —— person.go：演员页「TMDB 完整作品 + 简报」。
// 忠实移植桌面版 personTmdb.ts（lc-1033~1039）：
//
//	① fnOS /v/api/v1/person/{guid}（签名请求，cookie 由前端转发）→ imdbId / name
//	② fnOS /v/api/v1/person/item/list（job×5，page_size 200）→ 库内作品 owned 匹配键
//	   （规范化中/英标题 + tmdb_id；imdb→tmdb 兜底换算）
//	③ TMDB /find/{imdb} → person id → /person/{id}/combined_credits → 全量 cast
//	④ owned 标记 + 库内 guid 回填 → {ok, name, items, ownedCount, total}
//
// TMDB 请求复用 tmdbGet（v3/v4 鉴权 + 代理/免梯子直连）。缓存为进程内存 TTL（桌面版是
// userData 磁盘缓存 7d/30d + SWR；NAS 后端常驻，内存 6h 等效防 TMDB 限流）。
package bridge

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"regexp"
	"sort"
	"strings"
	"sync"
	"time"
)

var personGuidRe = regexp.MustCompile(`^[0-9a-f]{32}$`)

// jsNormTitle 标题归一化（桌面版 norm 同规则）：去大小写/空格/中英标点。
var jsNormRe = regexp.MustCompile(`[\s　·:：,，.。、!！?？'’"“”\-—_()（）[\]【】~～|/\\]`)

func jsNormTitle(s string) string {
	return jsNormRe.ReplaceAllString(strings.ToLower(s), "")
}

var deptCN = map[string]string{
	"Acting": "演员", "Directing": "导演", "Writing": "编剧", "Production": "制片",
	"Camera": "摄影", "Sound": "音效", "Art": "美术", "Editing": "剪辑", "Visual Effects": "视效",
}

// 内存缓存（成功结果才缓存；过期惰性删除）
var personCache sync.Map // key → personCacheEntry

type personCacheEntry struct {
	v   map[string]any
	exp time.Time
}

func personCacheGet(key string) map[string]any {
	if e, ok := personCache.Load(key); ok {
		en := e.(personCacheEntry)
		if time.Now().Before(en.exp) {
			return en.v
		}
		personCache.Delete(key)
	}
	return nil
}

func personCacheSet(key string, v map[string]any, ttl time.Duration) {
	personCache.Store(key, personCacheEntry{v: v, exp: time.Now().Add(ttl)})
}

// personLibItem 前端（同源签名 fetch）带回的库内作品行。
// [fix-person-tmdb] 网页端后端转发拿不到 httpOnly 会话 Cookie（Trim-MC-token），
// 直接由后端请求飞牛 person 详情会被判未登录 → data 为空 → 旧代码报「未找到该演员（guid）」。
// 因此改为前端用 credentials:'include' + Authx 抓取，后端只负责 TMDB 解析与 owned 匹配。
type personLibItem struct {
	Title  string  `json:"title"`
	Type   string  `json:"type"`
	GUID   string  `json:"guid"`
	TmdbID float64 `json:"tmdbId"`
	ImdbID string  `json:"imdbId"`
}

type personReq struct {
	GUID   string `json:"guid"`
	Cookie string `json:"cookie"`
	// 以下为可选提示（前端同源抓取结果）；缺失时后端回退直连飞牛。
	Name     string          `json:"name"`
	ImdbID   string          `json:"imdbId"`
	TmdbID   float64         `json:"tmdbId"`
	LibItems []personLibItem `json:"libItems"`
}

// personHints 解析后的提示（已清洗、限长）。
type personHints struct {
	GUID     string
	Name     string
	ImdbID   string
	TmdbID   int64
	LibItems []personLibRow
}

type personLibRow struct {
	Title  string
	Type   string
	GUID   string
	TmdbID int64
	ImdbID string
}

func (req personReq) hints() personHints {
	h := personHints{GUID: req.GUID}
	h.Name = strings.TrimSpace(req.Name)
	if len(h.Name) > 120 {
		h.Name = h.Name[:120]
	}
	h.ImdbID = strings.ToLower(strings.TrimSpace(req.ImdbID))
	if !imdbIDRe.MatchString(h.ImdbID) {
		h.ImdbID = ""
	}
	if req.TmdbID > 0 && req.TmdbID < 1e9 {
		h.TmdbID = int64(req.TmdbID)
	}
	for _, it := range req.LibItems {
		if len(h.LibItems) >= 600 {
			break
		}
		title := strings.TrimSpace(it.Title)
		if len(title) > 200 {
			title = title[:200]
		}
		imdb := strings.ToLower(strings.TrimSpace(it.ImdbID))
		if !imdbIDRe.MatchString(imdb) {
			imdb = ""
		}
		h.LibItems = append(h.LibItems, personLibRow{
			Title:  title,
			Type:   strings.ToLower(strings.TrimSpace(it.Type)),
			GUID:   strings.TrimSpace(it.GUID),
			TmdbID: int64(it.TmdbID),
			ImdbID: imdb,
		})
	}
	return h
}

// cacheKey 把解析提示并入缓存键：同名演员的不同 guid、以及「先无提示失败、后带提示成功」
// 不会互相污染。
func (h personHints) cacheKey() string {
	s := h.GUID
	if h.TmdbID > 0 {
		s += fmt.Sprintf("|t%d", h.TmdbID)
	}
	if h.ImdbID != "" {
		s += "|i" + h.ImdbID
	}
	if h.Name != "" {
		s += "|n" + jsNormTitle(h.Name)
	}
	return s
}

var imdbIDRe = regexp.MustCompile(`^tt[0-9]{5,12}$`)

// decodePersonReq 解析并校验 guid（32 位 hex，桌面版同规则）。
func decodePersonReq(r *http.Request) (personReq, string) {
	var req personReq
	// libItems 可能带来数百行作品，放宽到 512KB（仍远小于普通请求）。
	_ = json.NewDecoder(io.LimitReader(r.Body, 512*1024)).Decode(&req)
	guid := strings.ToLower(strings.TrimSpace(req.GUID))
	req.GUID = guid
	if !personGuidRe.MatchString(guid) {
		return req, "personGuid 无效"
	}
	return req, ""
}

// libMediaType 库内条目类型归一：tv/movie/空（桌面版同规则）。
func libMediaType(ty string) string {
	for _, k := range []string{"tv", "series", "show", "剧", "集"} {
		if strings.Contains(ty, k) {
			return "tv"
		}
	}
	for _, k := range []string{"movie", "film", "电影"} {
		if strings.Contains(ty, k) {
			return "movie"
		}
	}
	return ""
}

// personDetail 飞牛 person 详情（data 为对象才算成功）。
// 网页端后端转发通常拿不到 httpOnly 会话 Cookie，失败是常态；调用方应把 nil 视为
// 「详情不可用」并回退到前端提示/姓名搜索，而不是直接报错。
func (b *Bridge) personDetail(guid string, call fnOSCall) map[string]any {
	pd, err := b.callFnOSJSON(http.MethodGet, "/v/api/v1/person/"+guid, nil, call)
	if err != nil || pd == nil {
		return nil
	}
	p, _ := pd["data"].(map[string]any)
	return p
}

// personImdbID 从飞牛 person 详情解析 IMDb id（兼容 imdbId / imdb_id / 嵌套 person）。
func personImdbID(p map[string]any) string {
	if p == nil {
		return ""
	}
	v := jsFirstStr(p["imdb_id"], p["imdbId"], p["imdb"])
	if v == "" {
		if sub, _ := p["person"].(map[string]any); sub != nil {
			v = jsFirstStr(sub["imdb_id"], sub["imdbId"])
		}
	}
	v = strings.ToLower(strings.TrimSpace(v))
	if !imdbIDRe.MatchString(v) {
		return ""
	}
	return v
}

// personTmdbID 从飞牛 person 详情解析 TMDB person id
// （兼容 tmdb_id / tmdbId / tmdb / 嵌套 person / external_ids）。
func personTmdbID(p map[string]any) int64 {
	if p == nil {
		return 0
	}
	pick := func(m map[string]any) int64 {
		if m == nil {
			return 0
		}
		for _, k := range []string{"tmdb_id", "tmdbId", "tmdb_person_id"} {
			if n := int64(jsNum(m[k])); n > 0 && n < 1e9 {
				return n
			}
			// 部分版本用字符串存 id
			if s, ok := m[k].(string); ok {
				var n int64
				if _, err := fmt.Sscanf(strings.TrimSpace(s), "%d", &n); err == nil && n > 0 && n < 1e9 {
					return n
				}
			}
		}
		return 0
	}
	if n := pick(p); n > 0 {
		return n
	}
	for _, k := range []string{"person", "external_ids", "externalIds"} {
		if sub, _ := p[k].(map[string]any); sub != nil {
			if n := pick(sub); n > 0 {
				return n
			}
		}
	}
	return 0
}

// personLibItems 后端直连飞牛拉库内作品（前端未带 libItems 时的兜底；可能因会话 Cookie
// 缺失而为空，此时 owned 标记全部缺失，但 TMDB 作品列表仍可展示）。
func (b *Bridge) personLibItems(guid string, call fnOSCall) []personLibRow {
	var out []personLibRow
	for _, job := range []string{"Actor", "Director", "Writer", "Screenplay", "Producer"} {
		body, _ := json.Marshal(map[string]any{
			"person_guid": guid, "page": 1, "page_size": 200, "job": job,
			"sort_column": "update_time", "sort_type": "desc",
		})
		j, err := b.callFnOSJSON(http.MethodPost, "/v/api/v1/person/item/list", body, call)
		if err != nil {
			continue // 单 job 失败忽略
		}
		data, _ := j["data"].(map[string]any)
		list, _ := data["list"].([]any)
		for _, v := range list {
			m, ok := v.(map[string]any)
			if !ok {
				continue
			}
			tid := jsNum(m["tmdb_id"])
			if tid == 0 {
				tid = jsNum(m["tmdbId"])
			}
			out = append(out, personLibRow{
				Title:  jsFirstStr(m["title"], m["name"], m["original_title"], m["original_name"]),
				Type:   strings.ToLower(jsFirstStr(m["type"], m["media_type"])),
				GUID:   jsStr(m["guid"]),
				TmdbID: int64(tid),
				ImdbID: jsFirstStr(m["imdb_id"], m["imdbId"]),
			})
		}
	}
	return out
}

type ownedIndex struct {
	tmdb        map[string]bool
	titles      map[string]bool
	guidByTitle map[string]string
}

// buildOwnedIndex 库内作品 → owned 匹配键（规范化中/英标题 + tmdb_id；imdb→tmdb 兜底）。
func (b *Bridge) buildOwnedIndex(libItems []personLibRow) ownedIndex {
	ix := ownedIndex{
		tmdb:        map[string]bool{},
		titles:      map[string]bool{},
		guidByTitle: map[string]string{},
	}
	for _, it := range libItems {
		ty := libMediaType(it.Type)
		if it.Title != "" {
			key := jsNormTitle(it.Title)
			ix.titles[key] = true
			if it.GUID != "" {
				if _, ok := ix.guidByTitle[key]; !ok {
					ix.guidByTitle[key] = it.GUID
				}
			}
		}
		if it.TmdbID != 0 {
			if ty != "" {
				ix.tmdb[ty+fmt.Sprintf("%v", it.TmdbID)] = true
			} else { // 类型未知时两侧都记（桌面版同款兜底）
				ix.tmdb["tv"+fmt.Sprintf("%v", it.TmdbID)] = true
				ix.tmdb["movie"+fmt.Sprintf("%v", it.TmdbID)] = true
			}
		}
	}
	// imdb→tmdb 兜底（库内记录带 imdb_id 而标题没对上时的保险）
	// [perf-person] 旧实现逐行串行 /find，已有 tmdb_id 也照发、不去重、无缓存 → 慢。
	// 现改为：跳过已有 tmdb_id 的行 + imdb 去重 + 并发 6 路 + 24h 结果缓存。
	uniqImdb := make([]string, 0, len(libItems))
	seenImdb := make(map[string]bool, len(libItems))
	for _, it := range libItems {
		if it.ImdbID == "" || it.TmdbID != 0 || seenImdb[it.ImdbID] {
			continue
		}
		seenImdb[it.ImdbID] = true
		uniqImdb = append(uniqImdb, it.ImdbID)
	}
	if len(uniqImdb) > 0 {
		sem := make(chan struct{}, 6)
		var wg sync.WaitGroup
		var mu sync.Mutex
		for _, imdb := range uniqImdb {
			imdb := imdb
			wg.Add(1)
			sem <- struct{}{}
			go func() {
				defer wg.Done()
				defer func() { <-sem }()
				keys := b.findTmdbKeys(imdb)
				if len(keys) == 0 {
					return
				}
				mu.Lock()
				for _, k := range keys {
					ix.tmdb[k] = true
				}
				mu.Unlock()
			}()
		}
		wg.Wait()
	}
	return ix
}

// [perf-person] imdb→tmdb 索引键缓存（24h TTL，仅缓存成功查询；失败不缓存以便重试）。
var personFindCache sync.Map // imdb → personFindEntry

type personFindEntry struct {
	keys []string
	exp  time.Time
}

const personFindTTL = 24 * time.Hour

// findTmdbKeys 把 imdb_id 换成 owned 索引键（movie{id} / tv{id}），带 24h 缓存。
func (b *Bridge) findTmdbKeys(imdb string) []string {
	if e, ok := personFindCache.Load(imdb); ok {
		en := e.(personFindEntry)
		if time.Now().Before(en.exp) {
			return en.keys
		}
		personFindCache.Delete(imdb)
	}
	_, out, err := b.tmdbGet("/find/"+imdb, map[string]string{"external_source": "imdb_id"})
	if err != nil || out == nil {
		return nil
	}
	keys := make([]string, 0, 2)
	for _, mk := range []string{"movie_results", "tv_results"} {
		arr, _ := out[mk].([]any)
		prefix := "movie"
		if mk == "tv_results" {
			prefix = "tv"
		}
		for _, x := range arr {
			if xm, ok := x.(map[string]any); ok && xm["id"] != nil {
				keys = append(keys, prefix+fmt.Sprintf("%v", int64(jsNum(xm["id"]))))
			}
		}
	}
	personFindCache.Store(imdb, personFindEntry{keys: keys, exp: time.Now().Add(personFindTTL)})
	return keys
}

// resolveTmdbPersonID 把飞牛人物解析成 TMDB person id：
//
//	① 飞牛 person 详情的 tmdb_id（最精确）
//	② imdb_id → TMDB /find
//	③ 姓名 → TMDB /search/person（用库内已知作品校验 known_for，避免同名误配）
//
// 返回 (0, nil) 表示「确实没有关联」，调用方应展示「未关联 TMDB 人物」空状态；
// 返回 (0, err) 表示 TMDB 网络/镜像失败，调用方应展示可重试的错误。
func (b *Bridge) resolveTmdbPersonID(tmdbID int64, imdbID, name string, libItems []personLibRow) (int64, error) {
	if tmdbID > 0 {
		return tmdbID, nil
	}
	if imdbID != "" {
		st, find, err := b.tmdbGet("/find/"+imdbID, map[string]string{"external_source": "imdb_id"})
		if err != nil {
			return 0, err
		}
		if st == http.StatusOK && find != nil {
			if pr, _ := find["person_results"].([]any); len(pr) > 0 {
				if pm, ok := pr[0].(map[string]any); ok {
					if id := int64(jsNum(pm["id"])); id > 0 {
						return id, nil
					}
				}
			}
		}
		// IMDb 有值但 TMDB 未收录人物 → 继续按姓名兜底（旧版在这里直接放弃）
	}
	if name == "" {
		return 0, nil
	}
	return b.searchTmdbPerson(name, libItems)
}

// searchTmdbPerson 姓名搜索 TMDB 人物。先中文，再原始语言兜底。
// 匹配门槛：姓名精确匹配直接接受；否则必须与库内已知作品有 known_for 交集；
// 仅一个候选且姓名互相包含时也接受。其余情况视为未关联（宁缺勿错）。
func (b *Bridge) searchTmdbPerson(name string, libItems []personLibRow) (int64, error) {
	known := map[string]bool{}
	for _, it := range libItems {
		if it.Title != "" {
			known[jsNormTitle(it.Title)] = true
		}
	}
	nn := jsNormTitle(name)
	if nn == "" {
		return 0, nil
	}
	var lastErr error
	for _, noLang := range []bool{false, true} {
		st, out, err := b.tmdbGetOpts("/search/person", map[string]string{
			"query": name, "include_adult": "false",
		}, noLang)
		if err != nil {
			lastErr = err
			continue
		}
		if st != http.StatusOK || out == nil {
			continue
		}
		results, _ := out["results"].([]any)
		if len(results) == 0 {
			continue
		}
		var bestID int64
		var bestScore int
		var bestExact, bestOverlap bool
		for _, v := range results {
			m, ok := v.(map[string]any)
			if !ok {
				continue
			}
			id := int64(jsNum(m["id"]))
			if id <= 0 {
				continue
			}
			mn := jsNormTitle(jsFirstStr(m["name"], m["original_name"]))
			score := 0
			exact := mn != "" && mn == nn
			switch {
			case exact:
				score += 100
			case mn != "" && strings.Contains(mn, nn):
				score += 60
			case mn != "" && strings.Contains(nn, mn):
				score += 40
			}
			overlap := 0
			if kf, _ := m["known_for"].([]any); len(kf) > 0 {
				for _, kv := range kf {
					km, _ := kv.(map[string]any)
					if km == nil {
						continue
					}
					t := jsFirstStr(km["title"], km["name"], km["original_title"], km["original_name"])
					if t != "" && known[jsNormTitle(t)] {
						overlap++
					}
				}
				score += overlap * 30
			}
			if score > bestScore {
				bestScore, bestID, bestExact, bestOverlap = score, id, exact, overlap > 0
			}
		}
		if bestID > 0 {
			if bestExact || bestOverlap || (len(results) == 1 && bestScore >= 40) {
				return bestID, nil
			}
			return 0, nil // 有候选但不够可信：不猜
		}
	}
	if lastErr != nil {
		return 0, lastErr
	}
	return 0, nil
}

// personCreditsUncached 全量作品真抓取（桌面版 collectCreditsUncached 同逻辑）。
// [fix-person-tmdb] h 可携带前端同源抓取的 name/imdbId/tmdbId/libItems；只有缺失时才回退
// 到后端直连飞牛（网页端通常因 httpOnly Cookie 不可用而失败）。
func (b *Bridge) personCreditsUncached(h personHints, call fnOSCall) map[string]any {
	name, imdbID, tmdbID := h.Name, h.ImdbID, h.TmdbID

	// ① 飞牛 person 详情补全（name / imdb / tmdb id）
	if name == "" || (imdbID == "" && tmdbID == 0) {
		if p := b.personDetail(h.GUID, call); p != nil {
			if name == "" {
				name = jsFirstStr(p["name"], p["title"])
			}
			if imdbID == "" {
				imdbID = personImdbID(p)
			}
			if tmdbID == 0 {
				tmdbID = personTmdbID(p)
			}
		}
	}

	// ② 库内作品：优先用前端带回的行（后端直连常因会话缺失而空）
	libItems := h.LibItems
	if len(libItems) == 0 {
		libItems = b.personLibItems(h.GUID, call)
	}
	owned := b.buildOwnedIndex(libItems)

	// ③ 解析 TMDB person id（tmdb_id → imdb find → 姓名搜索）
	pid, err := b.resolveTmdbPersonID(tmdbID, imdbID, name, libItems)
	if err != nil {
		return map[string]any{
			// 原始错误（可能含镜像地址）只进日志；界面统一文案。
			"error":     publicError(err),
			"errorKind": "tmdb",
			"name":      name,
		}
	}
	if pid == 0 {
		return map[string]any{
			"ok": true, "unlinked": true, "name": name,
			"items": []any{}, "ownedCount": 0, "total": 0,
			"reason": personUnlinkedReason(imdbID, tmdbID, name),
		}
	}

	// ④ TMDB 全量 cast
	_, cr, err := b.tmdbGet("/person/"+fmt.Sprintf("%d", pid)+"/combined_credits", nil)
	if err != nil {
		return map[string]any{
			"error":     publicError(err),
			"errorKind": "tmdb",
			"name":      name,
		}
	}
	cast, _ := cr["cast"].([]any)
	seen := map[string]bool{}
	items := []map[string]any{}
	for _, v := range cast {
		m, ok := v.(map[string]any)
		if !ok || m["id"] == nil {
			continue
		}
		mt := jsStr(m["media_type"])
		if mt != "tv" {
			mt = "movie"
		}
		k := mt + fmt.Sprintf("%v", int64(jsNum(m["id"])))
		if seen[k] {
			continue
		}
		seen[k] = true
		title := jsFirstStr(m["title"], m["name"])
		if title == "" {
			title = "未知"
		}
		items = append(items, map[string]any{
			"id":        int64(jsNum(m["id"])),
			"title":     title,
			"en":        jsFirstStr(m["original_title"], m["original_name"]),
			"date":      jsFirstStr(m["release_date"], m["first_air_date"]),
			"character": jsStr(m["character"]),
			"score":     jsNum(m["vote_average"]),
			"poster":    jsStr(m["poster_path"]),
			"overview":  jsStr(m["overview"]),
			"media":     mt,
		})
	}
	sort.SliceStable(items, func(i, j int) bool {
		return jsStr(items[i]["date"]) > jsStr(items[j]["date"])
	})

	// ⑤ owned 标记 + 库内 guid 回填
	ownedCount := 0
	for _, m := range items {
		tKey := m["media"].(string) + fmt.Sprintf("%v", m["id"].(int64))
		nZh := jsNormTitle(m["title"].(string))
		en := m["en"].(string)
		if owned.tmdb[tKey] || owned.titles[nZh] || (en != "" && owned.titles[jsNormTitle(en)]) {
			m["owned"] = true
			if g, ok := owned.guidByTitle[nZh]; ok {
				m["guid"] = g
			} else if en != "" {
				if g, ok := owned.guidByTitle[jsNormTitle(en)]; ok {
					m["guid"] = g
				}
			}
			ownedCount++
		}
	}
	return map[string]any{
		"ok": true, "name": name, "items": items, "ownedCount": ownedCount,
		"total": len(items), "tmdbPersonId": pid,
	}
}

// personUnlinkedReason 给前端区分的空状态原因（不显示成「加载失败」）。
func personUnlinkedReason(imdbID string, tmdbID int64, name string) string {
	switch {
	case name == "":
		return "no-name"
	case imdbID == "" && tmdbID == 0:
		return "no-external-id"
	default:
		return "no-tmdb-match"
	}
}

// personBriefUncached 演员简报（职业分类/生日/出生地/代表作前二；桌面版 fetchBriefUncached 同逻辑）。
// [fix-person-tmdb] 与 credits 同一套 TMDB 人物解析（tmdb_id → imdb → 姓名搜索）。
func (b *Bridge) personBriefUncached(h personHints, call fnOSCall) map[string]any {
	name, imdbID, tmdbID := h.Name, h.ImdbID, h.TmdbID
	if name == "" || (imdbID == "" && tmdbID == 0) {
		if p := b.personDetail(h.GUID, call); p != nil {
			if name == "" {
				name = jsStr(p["name"])
			}
			if imdbID == "" {
				imdbID = personImdbID(p)
			}
			if tmdbID == 0 {
				tmdbID = personTmdbID(p)
			}
		}
	}
	brief := map[string]any{"ok": true, "name": name}

	// 简报是低优先级附加信息：不额外拉库内作品（避免季页每个演员 5 次飞牛请求），
	// 仅用前端可能带回的 libItems 做 known_for 校验。
	pid, err := b.resolveTmdbPersonID(tmdbID, imdbID, name, h.LibItems)
	if err != nil || pid == 0 {
		return brief // 简报尽力而为，失败/未关联不报错（桌面版同）
	}
	_, per, err := b.tmdbGet("/person/"+fmt.Sprintf("%d", pid), nil)
	if err == nil && per != nil {
		dep := jsStr(per["known_for_department"])
		if cn, ok := deptCN[dep]; ok {
			dep = cn
		}
		if dep != "" {
			brief["dept"] = dep
		}
		if v := jsStr(per["birthday"]); v != "" {
			brief["birthday"] = v
		}
		if v := jsStr(per["place_of_birth"]); v != "" {
			brief["place"] = v
		}
	}
	if _, cr, err := b.tmdbGet("/person/"+fmt.Sprintf("%d", pid)+"/combined_credits", nil); err == nil && cr != nil {
		if cast, _ := cr["cast"].([]any); len(cast) > 0 {
			type vc struct {
				title string
				count float64
			}
			var arr []vc
			for _, v := range cast {
				if m, ok := v.(map[string]any); ok {
					arr = append(arr, vc{jsFirstStr(m["title"], m["name"]), jsNum(m["vote_count"])})
				}
			}
			sort.SliceStable(arr, func(i, j int) bool { return arr[i].count > arr[j].count })
			if len(arr) > 2 {
				arr = arr[:2]
			}
			var top []string
			for _, it := range arr {
				if it.title == "" {
					continue
				}
				dup := false
				for _, t := range top {
					if t == it.title {
						dup = true
						break
					}
				}
				if !dup {
					top = append(top, it.title)
				}
			}
			if len(top) > 0 {
				brief["top"] = top
			}
		}
	}
	return brief
}

const personCreditsTTL = 6 * time.Hour
const personBriefTTL = 6 * time.Hour

// personCredits 演员页「TMDB 完整作品」POST {guid, cookie, name?, imdbId?, tmdbId?, libItems?}
// → {ok, name, items, ownedCount, total, tmdbPersonId?}；无法关联时 {ok:true, unlinked:true}。
func (b *Bridge) personCredits(w http.ResponseWriter, r *http.Request) {
	req, errMsg := decodePersonReq(r)
	if errMsg != "" {
		writeJSON(w, http.StatusOK, map[string]any{"error": errMsg})
		return
	}
	h := req.hints()
	key := "ptmdb_credits_v2_" + h.cacheKey()
	if c := personCacheGet(key); c != nil {
		writeJSON(w, http.StatusOK, c)
		return
	}
	out := b.personCreditsUncached(h, browserFnOSCall(r, req.Cookie))
	// 只缓存已解析出 TMDB 人物的成功结果；未关联/失败不缓存，允许下次带更完整提示重试。
	if out["error"] == nil && out["unlinked"] != true {
		personCacheSet(key, out, personCreditsTTL)
	}
	writeJSON(w, http.StatusOK, out)
}

// personBrief 演员简报 POST {guid, cookie, name?, imdbId?, tmdbId?, libItems?}
// → {ok, name, dept?, birthday?, place?, top?}。
func (b *Bridge) personBrief(w http.ResponseWriter, r *http.Request) {
	req, errMsg := decodePersonReq(r)
	if errMsg != "" {
		writeJSON(w, http.StatusOK, map[string]any{"error": errMsg})
		return
	}
	h := req.hints()
	key := "ptmdb_brief_v2_" + h.cacheKey()
	if c := personCacheGet(key); c != nil {
		writeJSON(w, http.StatusOK, c)
		return
	}
	out := b.personBriefUncached(h, browserFnOSCall(r, req.Cookie))
	if out["error"] == nil {
		personCacheSet(key, out, personBriefTTL)
	}
	writeJSON(w, http.StatusOK, out)
}
