/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useEffect, useRef, useState, type ChangeEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Clapperboard as IcoClapL, Film as IcoFilmL, ImagePlus as IcoImagePlusL, RotateCcw as IcoRotateL, Settings as IcoGearL, X as IcoXL } from "lucide-react";
import { PosterCard } from "../common/PosterCard";
import { ScrollRow } from "../common/ScrollRow";
import { CarouselView } from "./CarouselView";
import { pick } from "../../lib/format";
import { imgUrl, pickLargestImg } from "../../lib/images";
import { loadCustomCovers, resetLibraryCover, resolveDefaultCover, uploadLibraryCover } from "../../lib/libCovers";
import { ChromeTone, Lib, MediaItem, PlayItem, PlayTarget } from "../../types";

/** [lc-1646] 去掉标题里的装饰性音乐符号（飞牛单集标题里常见 ♪ ♫ 等），用户明确不要出现。 */
const stripDecor = (s: string) => s.replace(/[♪♫♬♩🎵🎶]/g, "").replace(/\s{2,}/g, " ").trim();


export function HomeView({
  libs,
  hiddenLibs,
  onToggleHiddenLib,
  continueList,
  carousel,
  carouselBds,
  carouselLogos,
  carouselLogoOn,
  launchPage,
  libRows,
  onOpenLib,
  onOpenFolder,
  onPlay,
  onOpenItem,
  onHeroTone,
  onEditMeta,
  onRefreshMeta,
}: {
  libs: Lib[];
  hiddenLibs: string[];
  onToggleHiddenLib: (guid: string) => void;
  continueList: PlayItem[];
  carousel: MediaItem[];
  carouselBds: Record<string, string>;
  carouselLogos: Record<string, string>;
  /** [lc-1438] 「轮播透明标题」开关：关时传空表让 CarouselView 走文字标题。
   *  不在这里过滤数据（数据留着，下次打开开关无需重拉）。 */
  carouselLogoOn: boolean;
  /** [lc-1439] 启动页设置：`"continue"` = 首页加载后自动滚到「继续观看」 */
  launchPage: "home" | "continue";
  libRows: { lib: Lib; items: MediaItem[]; total: number }[];
  onOpenLib: (lib: Lib) => void;
  /** [lc-1647] 首页横排里点到「未识别文件夹」→ 直接开库并钻到该层（个人视频/网盘库的根层就是文件夹） */
  onOpenFolder: (lib: Lib, folder: MediaItem) => void;
  onPlay: (t: PlayTarget) => void;
  onOpenItem: (item: MediaItem) => void;
  /** [lc-123-rev4] hero 图亮度采样结果上报（App → TitleBar 自适应反色） */
  onHeroTone?: (t: ChromeTone) => void;
  /** [lc-1654] 海报卡「编辑元数据 / 刷新元数据」由 App 注入（弹窗不在卡片内部） */
  onEditMeta?: (item: MediaItem) => void;
  onRefreshMeta?: (item: MediaItem) => void;
}) {
  const [manageOpen, setManageOpen] = useState(false);
  // [library-cover] 自定义封面（libGuid → /__cover/<file>）与默认横图封面（libGuid → 图片地址）
  const [customCovers, setCustomCovers] = useState<Record<string, string>>({});
  const [defaultCovers, setDefaultCovers] = useState<Record<string, string>>({});
  const [coverBusy, setCoverBusy] = useState<string | null>(null);
  const coverInputRef = useRef<HTMLInputElement | null>(null);
  const coverTargetLib = useRef<string | null>(null);
  const shownLibs = libs.filter((l) => !hiddenLibs.includes(l.guid));
  // [lc-1439] 启动页 = 继续观看：等「继续观看」区块渲染出来后把它滚到视口顶部。
  //   用 ref + scrollIntoView 而不是算像素：hero 是 100vh，偏移量随窗口高度变，
  //   算像素就得维护"hero 高度 + 媒体库条高度"这套常量（改版式必忘）。
  //   只在**首次**挂载后执行一次（ref 清空），否则用户手动滚回顶部又会被拽下去。
  const homeRef = useRef<HTMLElement | null>(null);
  const launchScrolled = useRef(false);
  useEffect(() => {
    if (launchScrolled.current || launchPage !== "continue") return;
    if (!continueList.length) return; // 没有观看记录就没得滚（保持首页顶部）
    launchScrolled.current = true;
    // 下一帧再滚：等这一轮 DOM 提交完（区块可能刚随 continueList 出现）
    const t = window.setTimeout(() => {
      const box = homeRef.current;
      const sec = box?.querySelector<HTMLElement>(".home-sec");
      if (box && sec) box.scrollTo({ top: sec.offsetTop, behavior: "auto" });
    }, 60);
    return () => window.clearTimeout(t);
  }, [launchPage, continueList.length]);

  // [lc-1431] **首页左缘对齐自检**（仅 dev）：启动后量四个关键元素的实际左边缘，
  //   写进诊断日志（dev_log → libmpv::trace）。
  //   为什么加这个：本轮我按 CSS 规范推演左缘，前后翻了两次结论（把 .content/.home-full
  //   当成父子、又漏算负边距），最后靠重新读 DOM 才定案 —— 推演不可靠，实测才可靠。
  //   这段代码在**真实渲染**后取值，日志里直接给出差值，以后改布局不必再猜。
  useEffect(() => {
    if (!import.meta.env.DEV) return;
    const t = window.setTimeout(() => {
      const leftOf = (sel: string): number | null => {
        const el = document.querySelector(sel);
        return el ? Math.round(el.getBoundingClientRect().left) : null;
      };
      const rows: [string, number | null][] = [
        ["轮播标题", leftOf(".mist-info h1")],
        ["轮播简介", leftOf(".mist-info p")],
        ["开始播放", leftOf(".mist-play")],
        ["媒体库条卡", leftOf(".lib-strip-card")],
        ["区块标题", leftOf(".home-sec .section-head h3")],
        ["横排首卡", leftOf(".hscroll .card")],
      ];
      const vals = rows.map(([, v]) => v).filter((v): v is number => v != null);
      const spread = vals.length ? Math.max(...vals) - Math.min(...vals) : -1;
      const detail = rows.map(([k, v]) => `${k}=${v ?? "-"}`).join(" ");
      invoke("dev_log", {
        msg: `[align] ${detail} | 极差=${spread}px${spread === 0 ? " ✅对齐" : " ⚠️不齐"}`,
      }).catch(() => {});
    }, 1500);
    return () => window.clearTimeout(t);
  }, []);

  // [library-cover] 启动时读回已保存的自定义封面映射（重启后仍生效）。
  useEffect(() => {
    let dead = false;
    loadCustomCovers()
      .then((map) => {
        if (!dead) setCustomCovers(map);
      })
      .catch(() => {});
    return () => {
      dead = true;
    };
  }, []);

  // [library-cover] 默认封面：复用首页已加载的库条目，取最新一条的横图（内部带 6h TTL 缓存）。
  //   每个库最多一次 get_item_detail（Rust 侧还有 120 天落盘缓存），不会随滚动重复请求。
  useEffect(() => {
    let dead = false;
    for (const { lib, items } of libRows) {
      if (!items.length) continue;
      resolveDefaultCover(lib.guid, items)
        .then((url) => {
          if (dead || !url) return;
          setDefaultCovers((prev) => (prev[lib.guid] === url ? prev : { ...prev, [lib.guid]: url }));
        })
        .catch(() => {});
    }
    return () => {
      dead = true;
    };
  }, [libRows]);

  const pickCover = (libGuid: string) => {
    coverTargetLib.current = libGuid;
    coverInputRef.current?.click();
  };

  const onCoverFile = async (e: ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    e.target.value = ""; // 允许连续选同一文件
    const libGuid = coverTargetLib.current;
    coverTargetLib.current = null;
    if (!file || !libGuid) return;
    if (!file.type.startsWith("image/")) {
      window.alert("请选择图片文件（JPG / PNG / WebP / GIF 等）");
      return;
    }
    if (file.size > 10 * 1024 * 1024) {
      window.alert("封面图片不能超过 10MB");
      return;
    }
    setCoverBusy(libGuid);
    try {
      const url = await uploadLibraryCover(libGuid, file);
      setCustomCovers((prev) => ({ ...prev, [libGuid]: url }));
    } catch (err) {
      window.alert(`更换封面失败：${String(err)}`);
    } finally {
      setCoverBusy(null);
    }
  };

  const restoreDefaultCover = async (libGuid: string) => {
    try {
      await resetLibraryCover(libGuid);
      setCustomCovers((prev) => {
        const next = { ...prev };
        delete next[libGuid];
        return next;
      });
    } catch (err) {
      window.alert(`恢复默认封面失败：${String(err)}`);
    }
  };

  return (
    <main className="content home-full" ref={homeRef}>
      {/* 全屏 hero 轮播 + 底部媒体库条（Emby 布局） */}
      <div className="hero-zone">
        <CarouselView
          items={carousel}
          bds={carouselBds}
          logos={carouselLogoOn ? carouselLogos : {}}
          onPlay={onPlay}
          onDetail={onOpenItem}
          onTone={onHeroTone}
        />
        <div className="lib-strip">
          {shownLibs.map((lib) => {
            // [library-cover] 优先级：自定义封面 → 最新条目横图 → 库自带横图 → 库海报。
            const cover =
              customCovers[lib.guid] ||
              defaultCovers[lib.guid] ||
              pickLargestImg(lib.backdrops) ||
              (lib.posters && lib.posters[0]) ||
              "";
            return (
              <div key={lib.guid} className="lib-strip-card" onClick={() => onOpenLib(lib)}>
                <div className="lsc-bg">
                  {cover ? (
                    <img src={imgUrl(cover)} loading="lazy" alt={lib.title} />
                  ) : (
                    <div className="poster-empty"><IcoClapL size={22} strokeWidth={1.8} /></div>
                  )}
                </div>
                <div className="lsc-shade" />
                <div className="lsc-title">{lib.title}</div>
                <div className="lsc-cat">{lib.category}</div>
                <button
                  type="button"
                  className="lsc-cover"
                  title={coverBusy === lib.guid ? "上传中…" : "更换封面"}
                  disabled={coverBusy === lib.guid}
                  onClick={(e) => {
                    e.stopPropagation();
                    pickCover(lib.guid);
                  }}
                >
                  <IcoImagePlusL size={13} strokeWidth={1.8} />
                </button>
                {customCovers[lib.guid] && (
                  <button
                    type="button"
                    className="lsc-cover lsc-cover-reset"
                    title="恢复默认封面"
                    onClick={(e) => {
                      e.stopPropagation();
                      void restoreDefaultCover(lib.guid);
                    }}
                  >
                    <IcoRotateL size={13} strokeWidth={1.8} />
                  </button>
                )}
                <button
                  type="button"
                  className="lsc-hide"
                  title="首页不展示该库"
                  onClick={(e) => {
                    e.stopPropagation();
                    onToggleHiddenLib(lib.guid);
                  }}
                >
                  <IcoXL size={13} strokeWidth={1.8} />
                </button>
              </div>
            );
          })}
          <div
            className={`lsc-manage ${shownLibs.length === 0 ? "always" : ""}`}
            title="管理首页媒体库展示"
            onClick={() => setManageOpen((v) => !v)}
          >
            <IcoGearL size={15} strokeWidth={1.8} /> 管理
          </div>
        </div>
        {/* [library-cover] 隐藏的文件选择器：卡片上的「更换封面」按钮触发 */}
        <input
          ref={coverInputRef}
          type="file"
          accept="image/*"
          style={{ display: "none" }}
          onChange={onCoverFile}
        />
        {manageOpen && (
          <>
            <div className="lsc-pop-mask" onClick={() => setManageOpen(false)} />
            <div className="lsc-pop">
              <div className="lsc-pop-head">首页媒体库展示</div>
              {libs.map((lib) => (
                <label key={lib.guid} className="lsc-pop-row">
                  <input
                    type="checkbox"
                    checked={!hiddenLibs.includes(lib.guid)}
                    onChange={() => onToggleHiddenLib(lib.guid)}
                  />
                  <span>{lib.title}</span>
                  <span className="lsc-pop-cat">{lib.category}</span>
                </label>
              ))}
            </div>
          </>
        )}
      </div>

      {/* 继续观看 */}
      {continueList.length > 0 && (
        <section className="home-sec">
          <div className="section-head">
            <h3>继续观看</h3>
          </div>
          <ScrollRow>
            {continueList
              .filter((raw) => String(pick(raw, ["type"]) ?? "") !== "LiveChannel")
              .map((raw) => {
              const guid = String(pick(raw, ["guid"]) ?? "");
              const rawTitle = stripDecor(String(pick(raw, ["title"]) ?? ""));
              const tvTitle = stripDecor(String(pick(raw, ["tv_title", "tvTitle"]) ?? ""));
              const type = String(pick(raw, ["type"]) ?? "");
              const poster = pick(raw, ["poster"]) as string | undefined;
              const ts = Number(pick(raw, ["ts"]) ?? 0);
              const duration = Number(pick(raw, ["duration"]) ?? 0);
              const sn = Number(pick(raw, ["season_number", "seasonNumber"]) ?? 0);
              const en = Number(pick(raw, ["episode_number", "episodeNumber"]) ?? 0);
              const watched = Number(pick(raw, ["watched"]) ?? 0) === 1;
              const isEpisode = type === "Episode";
              // [lc-1645] 卡片主标题永远是「剧名」；单集名（title）单独放第二行。
              // 飞牛 play/list 的 title 三种情况都见过：空串、"第 N 集"、真正的单集标题 ——
              // 之前只在 tv_title 存在时才渲染第二行，空串的条目（如药屋 S3E3）就整行缺失。
              const mainTitle = tvTitle || rawTitle;
              const progress = duration > 0 ? Math.min(100, (ts / duration) * 100) : 0;
              // 第二行「完整数据」：集号 + 单集标题（与剧名/集号重复时丢弃）+ 剩余时长
              const epLabel = isEpisode && en > 0 ? `第 ${en} 集` : "";
              const epTitle =
                rawTitle && rawTitle !== mainTitle && rawTitle !== epLabel ? rawTitle : "";
              const remainLabel =
                watched || (duration > 0 && ts >= duration)
                  ? "已看完"
                  : duration > 0 && ts > 0
                    ? `剩余 ${Math.max(1, Math.round((duration - ts) / 60))} 分钟`
                    : duration === 0 && ts > 0
                      ? `已看 ${Math.max(1, Math.round(ts / 60))} 分钟`
                      : "";
              const sub =
                [epLabel, epTitle, remainLabel].filter(Boolean).join(" · ") ||
                (type === "Movie" ? "电影" : "");
              return (
                <div
                  key={guid}
                  className="card continue-card"
                  onClick={() => {
                    // 点卡片空白处 → 进该剧详情；中间播放按钮单独直接播放
                    const parentGuid = String(
                      pick(raw, ["parent_guid", "parentGuid", "series_guid", "seriesGuid"]) ?? "",
                    );
                    onOpenItem({
                      guid: parentGuid || guid,
                      title: mainTitle,
                      tv_title: tvTitle || undefined,
                      // [lc-1774] guid 已归一到剧级，type 必须同步归一：集条目透传 "Episode"
                      // 会让详情页 isSeries 判否 → 整卡进「电影详情页」（剧数据+电影版式全错乱）
                      type: type === "Episode" || type === "Season" ? "TV" : type,
                      poster,
                      season_number: sn,
                      episode_number: en,
                      ts,
                      duration,
                    });
                  }}
                >
                  <div className="poster-box wide">
                    {poster ? (
                      <img src={imgUrl(poster)} loading="lazy" alt={mainTitle} />
                    ) : (
                      <div className="poster-empty"><IcoFilmL size={22} strokeWidth={1.8} /></div>
                    )}
                    {/* 中间播放按钮：只负责直接续播，阻止冒泡避免触发卡片进详情 */}
                    <button
                      type="button"
                      className="continue-play"
                      title="直接播放"
                      aria-label="直接播放"
                      onClick={(e) => {
                        e.stopPropagation();
                        onPlay({ guid, title: rawTitle || mainTitle, tvTitle });
                      }}
                    >
                      ▶
                    </button>
                    <div className="card-hover">
                      <span className="play-pill">▶ 续播</span>
                    </div>
                    {progress > 0 && (
                      <div className="progress">
                        <div style={{ width: `${progress}%` }} />
                      </div>
                    )}
                    {isEpisode && en > 0 && (
                      <span className="ep-badge">
                        S{sn > 0 ? sn : 1}E{en}
                      </span>
                    )}
                  </div>
                  <div className="card-title">{mainTitle}</div>
                  {sub && <div className="card-sub">{sub}</div>}
                </div>
              );
            })}
          </ScrollRow>
        </section>
      )}

      {/* 按库顺序的剧集横排（隐藏的库不展示） */}
      {libRows
        .filter(({ lib }) => !hiddenLibs.includes(lib.guid))
        .map(({ lib, items, total }) =>
        items.length > 0 ? (
          <section className="home-sec" key={lib.guid}>
            <div className="section-head">
              <h3>{lib.title}</h3>
              {/* [lc-1773] 库总数用服务端 total（分页后 items 恒为首页 60 条，
                  显示 items.length 会恒为 60）；total 缺失回落已加载数 */}
              <span className="muted">{total > 0 ? total : items.length} 项</span>
              <span className="row-more" onClick={() => onOpenLib(lib)}>
                全部 ›
              </span>
            </div>
            <ScrollRow center>
              {items.map((it) => (
                <PosterCard
                  key={it.guid}
                  item={it}
                  showMeta
                  onEditMeta={onEditMeta}
                  onRefreshMeta={onRefreshMeta}
                  onOpen={(x) =>
                    x.type === "Directory" ? onOpenFolder(lib, x) : onOpenItem(x)
                  }
                />
              ))}
            </ScrollRow>
          </section>
        ) : null,
      )}
    </main>
  );
}

/* ────────────────────────── 媒体库浏览 ────────────────────────── */
