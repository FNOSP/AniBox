/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */

export function DetailSkeleton() {
  return (
    <div className="dv2">
      <div className="dv2-left">
        <div className="dv2-top">
          <div className="dv2-poster"><div className="dsk dsk-dposter" /></div>
          <div className="dv2-topinfo">
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
        </div>
        <div className="dseg dv2-seg">
          {[0, 1, 2, 3, 4].map((i) => (
            <div key={i} className="dsk dsk-tab" />
          ))}
        </div>
      </div>
      <aside className="dv2-right">
        {/* [lc-1421] 骨架与真实头部同构：播放按钮在左、标题与元信息跟其后（一排） */}
        <div className="dv2e-hd">
          <div className="dsk dsk-playbtn" />
          <div className="dsk dsk-hd" />
          <div className="dsk dsk-hd2" />
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
