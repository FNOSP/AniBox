/* [lc-1701] 全局反馈工单未读轮询。
 *
 * 为什么放在 App 层而不是面板里：面板关闭时也要能提示「有新回复 / 状态已更新」，
 * 所以需要一条常驻、低频的轮询。这里复用 feedback_list（面板打开时本来就调它），
 * 每 12 小时一次；不做 visibilitychange 补拉，避免高频请求。
 * 面板内的会话轮询（20s）保持不变，两者不重复请求同一接口。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  ackUnread,
  computeUnread,
  FEEDBACK_POLL_MS,
  initReadStore,
  loadReadStore,
  markSeen,
  saveReadStore,
  syncReadStore,
  unreadBadgeCount,
  type FbListItem,
  type FbUnreadInfo,
} from "../lib/feedbackUnread";

export type FeedbackUnreadApi = {
  /** 工单列表（全局唯一来源；面板直接消费，避免重复请求） */
  items: FbListItem[] | null;
  listErr: string;
  /** 手动刷新（面板的「刷新」按钮 / 提交或回复后校正） */
  refresh: () => Promise<void>;
  /** 工单 id → 未读标记 */
  unread: Record<string, FbUnreadInfo>;
  /** 侧栏角标数字 */
  badge: number;
  /** 面板打开时调用：角标清零，行内标记保留 */
  ackPanel: () => void;
  /** 打开某个工单后调用：推进该工单的已读快照 */
  markTicketRead: (t: { id: string; status: string; lastTs: number | null }) => void;
  /** 应用内 toast 文案（空串 = 不显示） */
  toast: string;
};

export function useFeedbackUnread(panelOpen: boolean): FeedbackUnreadApi {
  const [items, setItems] = useState<FbListItem[] | null>(null);
  const [listErr, setListErr] = useState("");
  const [unread, setUnread] = useState<Record<string, FbUnreadInfo>>({});
  const [badge, setBadge] = useState(0);
  const [toast, setToast] = useState("");

  const itemsRef = useRef<FbListItem[] | null>(null);
  const storeRef = useRef(loadReadStore());
  const inFlight = useRef(false);
  const firstPoll = useRef(true);
  // 已经提醒过的「工单 + 事件」签名，避免同一事件每 60s 弹一次
  const notifiedRef = useRef<Set<string>>(new Set());
  const panelOpenRef = useRef(panelOpen);
  const toastTimer = useRef<number | null>(null);

  panelOpenRef.current = panelOpen;

  const showToast = useCallback((text: string) => {
    setToast(text);
    if (toastTimer.current !== null) window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(""), 4500);
  }, []);

  useEffect(
    () => () => {
      if (toastTimer.current !== null) window.clearTimeout(toastTimer.current);
    },
    [],
  );

  /** 重新计算未读 + 角标，并在必要时弹 toast */
  const applyStore = useCallback(
    (list: FbListItem[], notify: boolean) => {
      const store = storeRef.current;
      const map = computeUnread(list, store);
      setUnread(map);
      setBadge(unreadBadgeCount(map, store));

      if (!notify) {
        // 启动后的第一次同步只负责角标/行内标记，不弹 toast（避免重启即被历史未读打扰）
        for (const it of list) {
          const u = map[it.id];
          if (u) notifiedRef.current.add(u.sig);
        }
        return;
      }

      // 找一条最值得提醒的事件（优先新回复，其次状态更新），同签名只提醒一次
      let picked: { it: FbListItem; u: FbUnreadInfo } | null = null;
      for (const it of list) {
        const u = map[it.id];
        if (!u) continue;
        if (notifiedRef.current.has(u.sig)) continue;
        if (!picked || (u.newReply && !picked.u.newReply)) picked = { it, u };
      }
      if (!picked) return;
      for (const it of list) {
        const u = map[it.id];
        if (u) notifiedRef.current.add(u.sig);
      }
      // 面板已经打开时用户能直接看到行内标记，不再弹 toast
      if (panelOpenRef.current) return;

      const title = picked.it.title || "你的反馈";
      const short = title.length > 18 ? `${title.slice(0, 18)}…` : title;
      showToast(
        picked.u.newReply && picked.u.statusChanged
          ? `工单有新回复且状态已更新：${short}`
          : picked.u.newReply
            ? `工单有新回复：${short}`
            : `工单状态已更新：${short}`,
      );
    },
    [showToast],
  );

  const refresh = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    try {
      const list = await invoke<FbListItem[]>("feedback_list");
      const arr = Array.isArray(list) ? list : [];
      itemsRef.current = arr;
      setItems(arr);
      setListErr("");

      const prev = storeRef.current;
      if (!prev.initialized) {
        // 首次运行：当前列表整体作为已读基线，不产生未读
        storeRef.current = initReadStore(arr);
        saveReadStore(storeRef.current);
        setUnread({});
        setBadge(0);
        firstPoll.current = false;
        return;
      }

      const synced = syncReadStore(prev, arr);
      if (synced !== prev) {
        storeRef.current = synced;
        saveReadStore(synced);
      }
      applyStore(arr, !firstPoll.current);
      firstPoll.current = false;
    } catch (e) {
      // 网络失败只记录行内提示，保留已有列表与未读状态
      setListErr(String(e));
      setItems((prev) => prev ?? []);
    } finally {
      inFlight.current = false;
    }
  }, [applyStore]);

  // 常驻低频轮询：启动 1.5s 后首拉（避开启动高峰），之后每 12 小时一次。
  // 不做 visibilitychange 补拉，避免用户频繁切窗时变成高频请求。
  useEffect(() => {
    let cancelled = false;
    const tick = () => {
      if (cancelled) return;
      void refresh();
    };
    const first = window.setTimeout(tick, 1500);
    const iv = window.setInterval(tick, FEEDBACK_POLL_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(first);
      window.clearInterval(iv);
    };
  }, [refresh]);

  const ackPanel = useCallback(() => {
    const store = storeRef.current;
    const list = itemsRef.current;
    if (!store.initialized || !list) return;
    const map = computeUnread(list, store);
    const next = ackUnread(store, map);
    if (next === store) return;
    storeRef.current = next;
    saveReadStore(next);
    setBadge(unreadBadgeCount(map, next));
  }, []);

  const markTicketRead = useCallback(
    (t: { id: string; status: string; lastTs: number | null }) => {
      const store = storeRef.current;
      if (!store.initialized) return;
      const next = markSeen(store, t);
      storeRef.current = next;
      saveReadStore(next);

      // 会话返回的状态/时间通常比列表新：同步回列表项，避免「已读快照已前进、
      // 列表还是旧值」而被重新判定成状态变化（下一次轮询会用服务端值覆盖）。
      const current = itemsRef.current;
      const list = current
        ? current.map((it) =>
            it.id === t.id
              ? { ...it, status: t.status, lastTs: t.lastTs ?? it.lastTs }
              : it,
          )
        : null;
      if (list) {
        itemsRef.current = list;
        setItems(list);
      }

      const map = computeUnread(list ?? [], next);
      setUnread(map);
      setBadge(unreadBadgeCount(map, next));
    },
    [],
  );

  return { items, listErr, refresh, unread, badge, ackPanel, markTicketRead, toast };
}
