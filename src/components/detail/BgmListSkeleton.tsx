/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */

export function BgmListSkeleton() {
  return (
    <ul className="bgmc-list bgmc-skel" aria-hidden="true">
      {[0, 1, 2, 3].map((i) => (
        <li key={i} className="bgmc-item">
          <span className="bgmc-sk-avatar" />
          <div className="bgmc-main">
            <span className="bgmc-sk-line" style={{ width: i % 2 ? "40%" : "56%" }} />
            <span className="bgmc-sk-line" style={{ width: "94%" }} />
            <span className="bgmc-sk-line" style={{ width: i % 2 ? "72%" : "62%" }} />
          </div>
        </li>
      ))}
    </ul>
  );
}

/** Bangumi 角色/制作人员行（「更多」分段共用）：头像 + 名称 + 右侧角色/职位标签 + 可选副行（声优）。
 *  结构与「吐槽」列表同款（.bgmc-item 一族）；头像默认走 bgmImg（lain.bgm.tv 三级缓存代理），
 *  [lc-128] `img` 可传**已解析好的完整 URL**（TMDB 头像走 tmdbImg、飞牛兜底头像走 imgUrl），
 *  这样 TMDB / Bangumi / 飞牛三处演员表共用同一个展示组件，不新增一套。 */
