/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { IcoFolder, IcoGrid, IcoHome, IcoLive, IcoMovie, IcoStar, IcoTv } from "../../icons";
import { Clock as IcoClockL, MessageSquare as IcoMsgL, Settings as IcoGearL } from "lucide-react";


export const SIDEBAR_CATS = [
  { key: "全部", types: [] as string[], ico: <IcoGrid />, sumKey: "total" },
  { key: "电影", types: ["Movie"], ico: <IcoMovie />, sumKey: "movie" },
  { key: "电视节目", types: ["TV"], ico: <IcoTv />, sumKey: "tv" },
  { key: "电视直播", types: ["Live"], ico: <IcoLive />, sumKey: "live" },
  // [lc-1650] 「其他」必须带上 Directory：飞牛 mediadb/sum 的 video 计数把**未识别文件夹**
  // 也算进去了（实测 sum.video=2，而 tags.type=["Video"] 查出来 0 条、那 2 条其实是 Directory）。
  // 只查 Video 会出现「侧栏显示 2、点进去空的」。
  { key: "其他", types: ["Video", "Directory"], ico: <IcoFolder />, sumKey: "video" },
];


export function Sidebar({
  activeGuid,
  collapsed,
  username,
  server,
  sum,
  favTotal,
  onOpenFav,
  onOpenCat,
  onHome,
  onOpenSettings,
  onOpenFeedback,
  onOpenHistory,
}: {
  activeGuid: string | null;
  collapsed?: boolean;
  username?: string;
  server?: string;
  sum: Record<string, number>;
  favTotal: number | null;
  onOpenFav: () => void;
  onOpenCat: (cat: { key: string; types: string[] }) => void;
  onHome: () => void;
  /** [lc-128] 打开设置面板（缓存上限 / 占用 / 清理 + 关于） */
  onOpenSettings: () => void;
  /** [lc-1599] 打开反馈面板（工单制：提交 / 我的工单 / 会话） */
  onOpenFeedback: () => void;
  /** [lc-1615] 打开观影记录页（原「阶段3接入」toast 占位改真页面） */
  onOpenHistory: () => void;
}) {
  // 快捷操作卡折叠态：持久化键与主仓库 embyWall.ts 相同（'1'=收起成图标行，缺省=展开）
  const [cardFold, setCardFold] = useState(
    () => localStorage.getItem("fntvSidebarActions.collapsed") === "1",
  );
  const [appVer, setAppVer] = useState("0.1.0");
  useEffect(() => {
    getVersion().then(setAppVer).catch(() => {});
  }, []);
  function setFold(v: boolean) {
    setCardFold(v);
    try {
      localStorage.setItem("fntvSidebarActions.collapsed", v ? "1" : "0");
    } catch {
      /* ignore */
    }
  }
  return (
    <aside className={`sidebar ${collapsed ? "collapsed" : ""}`}>
      <div className={`side-item ${activeGuid === null ? "active" : ""}`} onClick={onHome}>
        <span className="side-ico">
          <IcoHome />
        </span>{" "}
        首页
      </div>
      <div className={`side-item ${activeGuid === "_fav" ? "active" : ""}`} onClick={onOpenFav}>
        <span className="side-ico">
          <IcoStar />
        </span>{" "}
        收藏
        <span className="side-count">{favTotal ?? ""}</span>
      </div>
      <div className="side-head">分类</div>
      {SIDEBAR_CATS.map((c) => {
        const cnt = sum[c.sumKey];
        return (
          <div key={c.key} className="side-item" onClick={() => onOpenCat(c)}>
            <span className="side-ico">{c.ico}</span> {c.key}
            <span className="side-count">{typeof cnt === "number" ? cnt : ""}</span>
          </div>
        );
      })}
      {/* 底部：用户行 + 快捷操作卡 —— 结构与样式逐行复刻主仓库 embyWall.ts injectSettingsUI */}
      <div className="side-userrow">
        <div className="side-avatar">{(username || "U").slice(0, 1).toUpperCase()}</div>
        <div className="side-usermeta">
          <div className="side-user-name">{username || "未登录"}</div>
          <div className="side-user-server">{server ? server.replace(/^https?:\/\//, "") : "未连接"}</div>
        </div>
      </div>
      <div className="side-actions">
        {cardFold ? (
          <div className="sa-icons">
            <button type="button" className="sa-ico" title="设置" onClick={onOpenSettings}>
              <IcoGearL size={15} strokeWidth={1.8} />
            </button>
            <button type="button" className="sa-ico" title="观影记录" onClick={onOpenHistory}>
              <IcoClockL size={15} strokeWidth={1.8} />
            </button>
            <button type="button" className="sa-ico" title="反馈" onClick={onOpenFeedback}>
              <IcoMsgL size={15} strokeWidth={1.8} />
            </button>
            <button type="button" className="sa-ico sa-expand" title="展开快捷操作" onClick={() => setFold(false)}>
              ∧
            </button>
          </div>
        ) : (
          <>
            <div className="sa-head" title="收起快捷操作" onClick={() => setFold(true)}>
              收起
              <span className="sa-chev" />
            </div>
            <button type="button" className="sa-btn" onClick={onOpenSettings}>
              <IcoGearL size={14} strokeWidth={1.8} /> 设置
            </button>
            <button type="button" className="sa-btn" onClick={onOpenHistory}>
              <IcoClockL size={14} strokeWidth={1.8} /> 观影记录
            </button>
            <button type="button" className="sa-btn" onClick={onOpenFeedback}>
              <IcoMsgL size={14} strokeWidth={1.8} /> 反馈
            </button>
            <div className="sa-ver">v{appVer}</div>
            {/* [lc-122] 数据来源署名（关于区=版本行下）：TMDB 条款强制原文 + Bangumi 署名。
                链接与详情页外链同款（<a target=_blank>），由 Tauri 交给系统浏览器打开。 */}
            <div className="sa-attrib">
              <span>
                This product uses the TMDB API but is not endorsed or certified by TMDB.
              </span>
              <span>
                <a href="https://www.themoviedb.org/" target="_blank" rel="noreferrer">
                  themoviedb.org
                </a>
              </span>
              <span>
                数据与部分内容来自{" "}
                <a href="https://bgm.tv/" target="_blank" rel="noreferrer">
                  Bangumi 番组计划
                </a>
              </span>
              {/* [lc-123-rev8] 图标库开源致谢（Lucide，ISC） */}
              <span>
                Icons by{" "}
                <a href="https://lucide.dev" target="_blank" rel="noreferrer">
                  Lucide (ISC)
                </a>
              </span>
            </div>
          </>
        )}
      </div>
    </aside>
  );
}

/* ────────────────────────── 设置面板（缓存 + 关于） ────────────────────────── */

/** [lc-128] 设置面板：目前两栏 —— **缓存**（上限 / 占用 / 清理）与**关于**（版本 + 署名，与侧栏
 *  关于区同一份文案）。外壳复用 Bangumi 弹层的 .ddet-mask/.ddet/.ddet-close/.ddet-hd 与 .ddet-body；
 *  内部 .set-* 全部走既有 CSS 变量（同一套深色亚克力玻璃、字体与圆角），不引入新配色。
 *  三要素对应 Rust 的三条命令：cache_stats / cache_set_limit / cache_clear
 *  （清理**同时清内存层与磁盘层**，不会出现"清了盘还留着旧图"）。 */
/** [lc-1437] 设置面板：通用 / 外观 / 账号 / 缓存 / 诊断 / 关于 六组。
 *  与 Rust 侧的分工：`get_settings`/`set_settings` 是**白名单子集**（不含密码/token），
 *  缓存那三条命令是独立的一小组；诊断三按钮走 `open_path`。 */
