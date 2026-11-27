/* [lc-2445] 社区通知未读角标（180s 低频轮询，照 useFeedbackUnread 的模式）。
 * 未登录/未部署服务端时静默降级为 0（不打扰）；面板打开时 CommunityPage 自己会立即刷新。 */
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { fetchSession } from "../lib/community";

/** 轮询间隔：社区通知不如工单紧急，且要控 D1 读行（design §7.6） */
const POLL_MS = 180 * 1000;

export function useCommunityUnread(): number {
  const [unread, setUnread] = useState(0);

  useEffect(() => {
    let alive = true;
    let timer: number | undefined;
    const poll = async () => {
      try {
        const s = await fetchSession();
        if (!alive) return;
        if (!s) {
          setUnread(0);
        } else {
          const r = await invoke<{ unread: number }>("community_v2_notifications", { cursor: 0 });
          if (!alive) return;
          setUnread(r?.unread ?? 0);
        }
      } catch {
        // 服务端未部署 / 断网：静默（角标保持原值）
      }
      if (alive) timer = window.setTimeout(poll, POLL_MS);
    };
    void poll();
    return () => {
      alive = false;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, []);

  return unread;
}
