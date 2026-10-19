/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。
   [lc-1568] 骨架跟随版面：标签/简介已移入「详情」页 —— 顶部信息框只留海报骨架，
   标签/简介的占位块下移到分段栏下方；分段占位 5 → 6（新增「更多」页）。 */

export function DetailSkeleton() {
  return (
    <div className="dv2">
      <div className="dv2-left">
        <div className="dv2-top">
          <div className="dv2-poster"><div className="dsk dsk-dposter" /></div>
          <div className="dv2-topinfo" />
        </div>
        <div className="dseg dv2-seg">
          {[0, 1, 2, 3, 4, 5].map((i) => (
            <div key={i} className="dsk dsk-tab" />
          ))}
        </div>
        {/* [lc-1568] 标签 / 简介 占位（对应「详情」页前两节） */}
        <div className="dv2-itags">
          {[0, 1, 2, 3, 4, 5].map((i) => (
            <div key={i} className="dsk dsk-chip" />
          ))}
        </div>
        <div className="dv2-isum">
          {[0, 1, 2, 3].map((i) => (
            <div key={i} className={`dsk dsk-sline${i === 3 ? " short" : ""}`} />
          ))}
        </div>
      </div>
      <aside className="dv2-right">
        {/* [选季重做] 真实头部现在是「第一层 flex-wrap + 第二层选季轨道」两层结构，
            骨架同样套一层 .dv2e-hd-top，避免三个占位块被纵向堆叠（高度会突变）。 */}
        <div className="dv2e-hd">
          <div className="dv2e-hd-top">
            {/* 与真实头部两行对齐：上面控件行，下面文本行 */}
            <div className="dv2e-hd-pills">
              <div className="dsk dsk-playbtn" />
              <div className="dsk dsk-hd2" />
            </div>
            <div className="dv2e-hd-meta">
              <div className="dsk dsk-hd" />
            </div>
          </div>
        </div>
        <div className="dv2e-list">
          {[0, 1, 2].map((i) => (
            <div className="dv2e-row" key={i}>
              <div className="dv2e-thumb"><div className="dsk dsk-dthumb" /></div>
              <div className="dv2e-main">
                <div className="dsk dsk-sline" />
                <div className={`dsk dsk-sline${i % 2 ? " short" : ""}`} />
                <div className="dsk dsk-sline short" />
              </div>
            </div>
          ))}
        </div>
      </aside>
    </div>
  );
}

/* ────────────────────────── 首页骨架屏 ────────────────────────── */
