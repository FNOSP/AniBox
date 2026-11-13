/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useEffect, useState } from "react";
import { DUR } from "../lib/motion";

/* [lc-2135] 默认时长跟随 --dur-move（页窗退场 viewOut 也用这一档）。
   两处必须同步：CSS 调快点、这里还在空等，观感就是"点了返回没反应"。 */
export function useDelayedUnmount(active: boolean, ms = DUR.move) {
  const [mounted, setMounted] = useState(active);
  const [closing, setClosing] = useState(false);
  useEffect(() => {
    if (active) {
      setMounted(true);
      setClosing(false);
      return;
    }
    if (!mounted) return;
    setClosing(true);
    const t = window.setTimeout(() => {
      setMounted(false);
      setClosing(false);
    }, ms);
    return () => window.clearTimeout(t);
  }, [active, mounted, ms]);
  return [mounted, closing] as const;
}
