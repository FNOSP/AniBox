/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Clapperboard as IcoClapL, Film as IcoFilmL, Settings as IcoGearL, X as IcoXL } from "lucide-react";
import { PosterCard } from "../common/PosterCard";
import { ScrollRow } from "../common/ScrollRow";
import { CarouselView } from "./CarouselView";
import { pick } from "../../lib/format";
import { imgUrl } from "../../lib/images";
import { ChromeTone, Lib, MediaItem, PlayItem, PlayTarget } from "../../types";


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
  onPlay,
  onOpenItem,
  onHeroTone,
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
  libRows: { lib: Lib; items: MediaItem[] }[];
  onOpenLib: (lib: Lib) => void;
  onPlay: (t: PlayTarget) => void;
  onOpenItem: (item: MediaItem) => void;
  /** [lc-123-rev4] hero 图亮度采样结果上报（App → TitleBar 自适应反色） */
  onHeroTone?: (t: ChromeTone) => void;
}) {
  const [manageOpen, setManageOpen] = useState(false);
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
          {shownLibs.map((lib) => (
            <div key={lib.guid} className="lib-strip-card" onClick={() => onOpenLib(lib)}>
              <div className="lsc-bg">
                {lib.posters && lib.posters[0] ? (
                  <img src={imgUrl(lib.posters[0])} loading="lazy" alt={lib.title} />
                ) : (
                  <div className="poster-empty"><IcoClapL size={22} strokeWidth={1.8} /></div>
                )}
              </div>
              <div className="lsc-shade" />
              <div className="lsc-title">{lib.title}</div>
              <div className="lsc-cat">{lib.category}</div>
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
          ))}
          <div
            className={`lsc-manage ${shownLibs.length === 0 ? "always" : ""}`}
            title="管理首页媒体库展示"
            onClick={() => setManageOpen((v) => !v)}
          >
            <IcoGearL size={15} strokeWidth={1.8} /> 管理
          </div>
        </div>
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
            {continueList.map((raw) => {
              const guid = String(pick(raw, ["guid"]) ?? "");
              const title = String(pick(raw, ["title"]) ?? "");
              const tvTitle = pick(raw, ["tvTitle", "tv_title"]) as string | undefined;
              const poster = pick(raw, ["poster"]) as string | undefined;
              const ts = Number(pick(raw, ["ts"]) ?? 0);
              const duration = Number(pick(raw, ["duration"]) ?? 0);
              const sn = pick(raw, ["seasonNumber", "season_number"]);
              const en = pick(raw, ["episodeNumber", "episode_number"]);
              const progress = duration > 0 ? Math.min(100, (ts / duration) * 100) : 0;
              return (
                <div
                  key={guid}
                  className="card continue-card"
                  onClick={() => onPlay({ guid, title, tvTitle })}
                >
                  <div className="poster-box wide">
                    {poster ? (
                      <img src={imgUrl(poster)} loading="lazy" alt={title} />
                    ) : (
                      <div className="poster-empty"><IcoFilmL size={22} strokeWidth={1.8} /></div>
                    )}
                    <div className="card-hover">
                      <span className="play-pill">▶ 续播</span>
                    </div>
                    {progress > 0 && (
                      <div className="progress">
                        <div style={{ width: `${progress}%` }} />
                      </div>
                    )}
                    {sn != null && (
                      <span className="ep-badge">
                        S{sn}E{en ?? ""}
                      </span>
                    )}
                  </div>
                  <div className="card-title">{tvTitle || title}</div>
                  {tvTitle && <div className="card-sub">{title}</div>}
                </div>
              );
            })}
          </ScrollRow>
        </section>
      )}

      {/* 按库顺序的剧集横排（隐藏的库不展示） */}
      {libRows
        .filter(({ lib }) => !hiddenLibs.includes(lib.guid))
        .map(({ lib, items }) =>
        items.length > 0 ? (
          <section className="home-sec" key={lib.guid}>
            <div className="section-head">
              <h3>{lib.title}</h3>
              <span className="muted">{items.length} 项</span>
              <span className="row-more" onClick={() => onOpenLib(lib)}>
                全部 ›
              </span>
            </div>
            <ScrollRow>
              {items.map((it) => (
                <PosterCard key={it.guid} item={it} onOpen={onOpenItem} />
              ))}
            </ScrollRow>
          </section>
        ) : null,
      )}
    </main>
  );
}

/* ────────────────────────── 媒体库浏览 ────────────────────────── */
