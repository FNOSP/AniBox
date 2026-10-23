/* [lc-1728] 全局轻提示宿主：挂在 App 根部，渲染 showAppToast() 派发的 toast（portal 到 body）。
   无阻塞、自动消失（3.5s），多条时纵向堆叠。样式复用 .app-toast（33-feedback.css，与 fb-toast 同外观）。 */
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { onAppToast, type AppToastItem } from "../../lib/metaApply";

let seq = 0;

export function AppToastHost() {
  const [items, setItems] = useState<AppToastItem[]>([]);

  useEffect(() => {
    const timers = new Map<number, number>();
    const off = onAppToast((text) => {
      const id = ++seq;
      setItems((prev) => [...prev.slice(-2), { id, text }]);
      timers.set(
        id,
        window.setTimeout(() => {
          setItems((prev) => prev.filter((t) => t.id !== id));
          timers.delete(id);
        }, 3500),
      );
    });
    return () => {
      off();
      timers.forEach((t) => window.clearTimeout(t));
      timers.clear();
    };
  }, []);

  if (!items.length) return null;
  return createPortal(
    <div className="app-toast-wrap" role="status" aria-live="polite">
      {items.map((t) => (
        <div key={t.id} className="app-toast">
          {t.text}
        </div>
      ))}
    </div>,
    document.body,
  );
}
