/* [lc-1535] 从 App.tsx 拆出（纯搬移，逻辑与注释未改动）。 */
import { useEffect, useState } from "react";

export function useDelayedUnmount(active: boolean, ms = 200) {
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
