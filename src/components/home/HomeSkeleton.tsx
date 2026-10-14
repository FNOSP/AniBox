/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */


export function HomeSkeleton() {
  return (
    <main className="content home-full">
      <div className="skel hero-skel" />
      <div className="skel-row">
        {Array.from({ length: 5 }).map((_, i) => (
          <div key={i} className="skel lsc-skel" />
        ))}
      </div>
      {[0, 1].map((r) => (
        <section className="home-sec" key={r}>
          <div className="skel sec-title-skel" />
          <div className="skel-row">
            {Array.from({ length: 6 }).map((_, i) => (
              <div key={i} className="skel card-skel" />
            ))}
          </div>
        </section>
      ))}
    </main>
  );
}

/* ────────────────────────── 首页 ────────────────────────── */
