/* [lc-2294] 短剧栏首屏加载态：进度条 + 网格骨架。
 *
 * 起因：lc-2235「缓存优先」把「请求开始就置 busy」去掉了 —— 在 cached/catalog 回包之前
 * busy=false 且 items=[] 会**直接落到「暂无内容」空态**，用户看到的就是「短剧页全空了」。
 * 现在「数据未落定」由 DuanjuPane 的 boot 阶段单独表达，本组件只负责把它画出来：
 *   · 进度条按真实里程碑走（连接核心 25% → 读取目录 70%），不伪造百分比；
 *   · 骨架形状对齐 .dj-grid / .dj-card（2:3 海报 + 两行文字），数据到了不跳动；
 *   · 超过 5 秒补一行「站源响应较慢」—— 红果冷启动实测 6.4s，光有骨架仍像卡死。
 */
import { useEffect, useState } from "react";
import { Dsk } from "./shared";

/** 骨架卡片数：一屏视口大约铺满这么多（2:3 竖版，118px 起） */
const SK_COUNT = 12;
/** 超过这个秒数才提示「较慢」，短于它的正常加载不该被描述成慢 */
const SLOW_HINT_SEC = 5;

export function DuanjuLoading({ progress, label }: { progress: number; label: string }) {
  const [elapsed, setElapsed] = useState(0);
  useEffect(() => {
    const t0 = Date.now();
    const id = window.setInterval(() => setElapsed(Math.floor((Date.now() - t0) / 1000)), 1000);
    return () => window.clearInterval(id);
  }, []);

  return (
    <div className="dj-loading">
      <div
        className="dj-loadbar"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={progress}
        aria-label={label}
      >
        <div className="dj-loadbar-fill" style={{ width: `${progress}%` }} />
      </div>
      <div className="dj-loading-t">
        <span>{label}</span>
        <span className="dj-loading-pct">{progress}%</span>
      </div>
      {elapsed >= SLOW_HINT_SEC && (
        <div className="dj-loading-hint">站源响应较慢，正在继续加载…</div>
      )}
      <div className="dj-grid" aria-hidden="true">
        {Array.from({ length: SK_COUNT }, (_, i) => (
          <div className="dj-skcard" key={i}>
            <div className="dj-skposter">
              <Dsk cls="dsk-poster" />
            </div>
            <Dsk cls="dsk-line" />
            <Dsk cls="dsk-line short" />
          </div>
        ))}
      </div>
    </div>
  );
}
