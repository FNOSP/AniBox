/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { Film as IcoFilmL } from "lucide-react";
import { bgmImg } from "../../lib/images";
import { BgmCalendarItem } from "../../types";

export function BgmCalRow({
  it,
  today,
  rank,
  libState,
  onOpen,
}: {
  it: BgmCalendarItem;
  today: boolean;
  /** 该天列表内的序号（1 起，按上游顺序，就是封面左上角那个角标） */
  rank: number;
  /** "in" = 已匹配到飞牛库（打「已在库」标）；null = 未匹配或索引不可用（不标） */
  libState: "in" | null;
  onOpen: (it: BgmCalendarItem) => void;
}) {
  const title = it.name_cn || it.name || "（无标题）";
  const year = (it.air_date || "").slice(0, 4);
  const score = typeof it.score === "number" && isFinite(it.score) && it.score > 0 ? it.score : null;
  const tier = score == null ? null : score >= 8 ? "hi" : score >= 7 ? "mid" : "lo";
  const tip = [title, year ? `${year} 年` : "", score != null ? `评分 ${score.toFixed(1)}` : ""]
    .filter(Boolean)
    .join(" · ");
  return (
    <li
      className="ditem"
      role="button"
      tabIndex={0}
      title={`${tip} · 点击查看 Bangumi 详情`}
      onClick={() => onOpen(it)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onOpen(it);
        }
      }}
    >
      <div className={`dcv${it.cover ? "" : " empty"}`}>
        {it.cover ? (
          <img src={bgmImg(it.cover)} alt="" loading="lazy" width={46} height={62} draggable={false} />
        ) : (
          <span aria-hidden="true"><IcoFilmL size={18} strokeWidth={1.8} /></span>
        )}
        <span className="drk" aria-hidden="true">
          {rank}
        </span>
      </div>
      <div className="dtx">
        <div className="dt1">{title}</div>
        <div className="dt2">
          {tier && score != null && <span className={`dsc ${tier}`}>{score.toFixed(1)}</span>}
          <span>{year ? `${year} 年` : "年份未知"}</span>
          {libState === "in" && <span className="dlib">已在库</span>}
          {today && <span className="ditem-tag">今日更新</span>}
        </div>
      </div>
    </li>
  );
}

/** 右栏：Bangumi 新番更新（7 个星期胶囊<迷你占比条> + 当天汇总条 + 当日列表）。
 *  点击条目 → 打开该 Bangumi 条目的轻量详情。
 *  [lc-123-rev] 在库分组：只对**当前选中这天**的条目做匹配（不逐条打网络），在库在前、组标题带数量；
 *  索引不完整（partial）时退化成"只给命中项打标"，索引不可用时整个分组退场（见 useLibIndex 注释）。 */
/* ════════════ [lc-1516] 发现页右栏顶部：Bangumi 追番同步卡 ════════════
   版式按用户设计稿：左侧 = 图标 +「Bangumi / 追番同步」标题组 + 描述 + 状态枚举
   + 大按钮（连接 / 立即同步）；右侧 = 观看统计（追番状态 donut + 分项计数）。
   数据源：Rust bgm_account 模块（走自建镜像，见 bgm_account.rs）。 */
