/* [lc-1536] 从 DetailView 拆出的标签页（纯搬移，逻辑与注释未改动）。 */
import { BgmListSkeleton } from "../BgmListSkeleton";
import { BgmCastRow } from "../BgmCastRow";
import { tmdbImg, imgUrl } from "../../../lib/images";
import { CHAR_TYPE_LABEL, fnosCast } from "../helpers";
import { BgmCharacter, BgmPerson, PersonTarget, TmdbCreditPerson, TmdbCredits } from "../../../types";

type Props = {
  cast: any[];
  castFrom: any;
  showCast: any;
  chars: BgmCharacter[];
  credits: TmdbCredits | null;
  creditsBusy: boolean;
  creditsCast: TmdbCreditPerson[];
  creditsErr: string;
  crew: any[];
  fbCast: ReturnType<typeof fnosCast>;
  moreBusy: boolean;
  moreErr: string;
  moreProbed: boolean;
  persons: BgmPerson[];
  retryCredits: () => void;
  retryMore: () => void;
  /** [lc-1700] 点人物 → 打开演员页（TMDB cast/crew、BGM 角色声优/制作人员） */
  onOpenPerson?: (t: PersonTarget) => void;
};
export function CastTab({ cast, castFrom, showCast, chars, credits, creditsBusy, creditsCast, creditsErr, crew, fbCast, moreBusy, moreErr, moreProbed, persons, retryCredits, retryMore, onOpenPerson }: Props) {
  /** TMDB 人物（cast / crew 通用）：id > 0 才可点 */
  const openTmdb = (c: any) => {
    const id = Number(c?.id ?? 0);
    if (!onOpenPerson || !(id > 0)) return undefined;
    return () =>
      onOpenPerson({
        source: "tmdb",
        id,
        name: String(c?.name ?? ""),
        profile: c?.profile ? String(c.profile) : undefined,
      });
  };
  /** Bangumi 人物：id 存在才可点 */
  const openBgm = (id: unknown, name: unknown, profile?: unknown) => {
    const n = Number(id ?? 0);
    if (!onOpenPerson || !(n > 0)) return undefined;
    return () =>
      onOpenPerson({
        source: "bgm",
        id: n,
        name: String(name ?? ""),
        profile: profile ? String(profile) : undefined,
      });
  };
  return (
    <>
      <div className="dpanel dmore">
        {/* (1) 演员表（TMDB 主源）：骨架 / 失败重试 / 数据 */}
        {creditsBusy && !credits && !creditsErr && <BgmListSkeleton />}
        {creditsErr && !creditsCast.length && !showCast.length && (
          <div className="bgmc-state">
            <div className="bgmc-state-t">演员表（TMDB）加载失败</div>
            <div className="bgmc-state-d">{creditsErr}</div>
            <button className="bgmc-retry" onClick={retryCredits}>
              重试
            </button>
          </div>
        )}
        {cast.length > 0 && (
          <section className="dsec">
            <h2 className="dsec-t">
              演职人员
              <span className="dsec-src" title={credits?.matchedBy || (castFrom === "details" ? "TMDB 详情接口" : "")}>
                TMDB{credits?.tmdbId ? ` #${credits.tmdbId}` : ""}
                {credits?.cached ? " · 本地缓存" : ""}
              </span>
            </h2>
            <ul className="bgmc-list">
              {cast.map((c, i) => (
                <BgmCastRow
                  key={`${c.id ?? c.name}-${i}`}
                  name={c.name}
                  tag={c.character ? `饰 ${c.character}` : ""}
                  sub={Number(c.episodes ?? 0) > 0 ? `出演 ${c.episodes} 集` : ""}
                  img={c.profile ? tmdbImg(String(c.profile), "w185") : ""}
                  fallback="未知演员"
                  onClick={openTmdb(c)}
                />
              ))}
            </ul>
          </section>
        )}
        {crew.length > 0 && (
          <section className="dsec">
            <h2 className="dsec-t">
              制作
              <span className="dsec-src">TMDB</span>
            </h2>
            <ul className="bgmc-list">
              {crew.map((c, i) => (
                <BgmCastRow
                  key={`${c.id ?? c.name}-${i}`}
                  name={c.name}
                  tag={c.jobLabel || c.job || ""}
                  sub={Number(c.episodes ?? 0) > 0 ? `${c.episodes} 集` : ""}
                  img={c.profile ? tmdbImg(String(c.profile), "w185") : ""}
                  fallback="未知"
                  onClick={openTmdb(c)}
                />
              ))}
            </ul>
          </section>
        )}
        {/* (2) 飞牛演员字段（**只兜底**：TMDB 完全没数据时才显示；没有该字段则整块不出现） */}
        {!creditsBusy && !cast.length && !crew.length && fbCast.length > 0 && (
          <section className="dsec">
            <h2 className="dsec-t">
              演职人员
              <span className="dsec-src">飞牛</span>
            </h2>
            <ul className="bgmc-list">
              {fbCast.map((c, i) => (
                <BgmCastRow
                  key={`${c.name}-${i}`}
                  name={c.name}
                  tag={c.role ? `饰 ${c.role}` : ""}
                  img={c.avatar ? (c.avatar.startsWith("http") ? c.avatar : imgUrl(c.avatar)) : ""}
                  fallback="未知演员"
                />
              ))}
            </ul>
          </section>
        )}
        {moreBusy && !moreErr && <BgmListSkeleton />}
        {moreErr && (
          <div className="bgmc-state">
            <div className="bgmc-state-t">角色与制作人员加载失败</div>
            <div className="bgmc-state-d">{moreErr}</div>
            <button className="bgmc-retry" onClick={retryMore}>
              重试
            </button>
          </div>
        )}
        {!moreBusy &&
          !moreErr &&
          moreProbed &&
          chars.length === 0 &&
          persons.length === 0 &&
          !creditsBusy &&
          cast.length === 0 &&
          crew.length === 0 &&
          fbCast.length === 0 && <div className="empty">暂无角色与制作人员信息</div>}
        {chars.length > 0 && (
          <section className="dsec">
            <h2 className="dsec-t">
              角色
              <span className="dsec-src">Bangumi</span>
            </h2>
            <ul className="bgmc-list">
              {chars.map((c, i) => (
                <BgmCastRow
                  key={String(c.id ?? i)}
                  name={c.name}
                  tag={c.relation || CHAR_TYPE_LABEL[Number(c.type)] || ""}
                  sub={c.actors?.[0]?.name ? `CV：${c.actors[0].name}` : ""}
                  avatar={c.images?.large || c.images?.grid || c.images?.medium || c.images?.small}
                  fallback="未知角色"
                  onClick={openBgm(c.actors?.[0]?.id, c.actors?.[0]?.name, c.actors?.[0]?.images?.grid)}
                />
              ))}
            </ul>
          </section>
        )}
        {/* 制作人员（Bangumi）：TMDB 已有 crew 时不再重复展示同一批人 */}
        {persons.length > 0 && crew.length === 0 && (
          <section className="dsec">
            <h2 className="dsec-t">
              制作人员
              <span className="dsec-src">Bangumi</span>
            </h2>
            <ul className="bgmc-list">
              {persons.map((p, i) => (
                <BgmCastRow
                  key={String(p.id ?? i)}
                  name={p.name}
                  tag={p.relation || (Array.isArray(p.career) ? p.career.join(" / ") : "")}
                  avatar={p.images?.large || p.images?.grid || p.images?.medium || p.images?.small}
                  fallback="未知"
                  onClick={openBgm(p.id, p.name, p.images?.grid)}
                />
              ))}
            </ul>
          </section>
        )}
      </div>
    </>
  );
}
