/* [progress] 全局播放进度上报（与旧项目 Fntv-Plus 的「进度回传 + 继续观看」链路对齐）。
 *
 * 为什么必须全局：进度/继续观看不能依赖「观影记录」页挂载 —— 用户正常播放时不会进那个页面。
 * 本 hook 在 App 顶层无条件挂载，只监听 mpv 事件（Rust 侧 mpv.rs 发出）：
 *
 *   事件来源（mpv.rs / player.rs 实际发出）：
 *     · `mpv-load {phase:"start"}`  起播 / 整季播放列表切到下一集（mpv start-file）
 *     · `mpv-load {phase:"ended"}`  正常结束（end-file）/ mpv 进程退出（IPC 断开）
 *     · `mpv-end`                   播放失败
 *     · `mpv-event` id=1 time-pos / 2 duration / 3 pause / 10 playlist-pos / 13 filename
 *     · `history-recorded {guid,source}` 起播时后端落库后广播（本地记录键）
 *
 * 上报节奏：
 *     · 起播后**第一个 time-pos > 0** 立即上报一次（继续观看马上出现）；
 *     · 播放中每 10s 一次；
 *     · 暂停 / 切集（playlist-pos 变化）/ 结束 / mpv 退出 / 页面隐藏 立即 flush；
 *     · 结束时再用 `record_watch_session` 回填真实观看时长（年度报告用）。
 *
 * 两条链路（后端 `report_watch_progress` 内部）：
 *     ① 本地 history.json 按剧级 series_key 更新（不新增重复记录）；
 *     ② 飞牛 `POST /v/api/v1/play/record` —— 首页「继续观看」的服务端数据源。
 */
import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** 进度上报间隔：10s（写盘 + 飞牛回传节流；其余时机另有立即 flush） */
const PROGRESS_REPORT_MS = 10_000;

type HistoryRecorded = {
  guid?: string;
  source?: string;
  seriesKey?: string;
};

type MpvEvent = {
  event?: string;
  id?: number;
  data?: unknown;
};

/** 从代理直链 `/api/v1/playvideo/<guid>?...` 解析当前集 guid（旧项目同做法）。 */
function parsePlayGuid(url: string): string {
  const mark = "/api/v1/playvideo/";
  const idx = url.indexOf(mark);
  if (idx < 0) return "";
  const rest = url.slice(idx + mark.length);
  return rest.split(/[?#/]/)[0] || "";
}

export function usePlaybackProgress(): void {
  useEffect(() => {
    /** 会话状态（effect 内单实例；组件卸载即失效） */
    const s = {
      /** 本地历史记录键（history-recorded）；整季自动切集时保持不变 */
      guid: "",
      /** 当前正在播的那一集 guid（mpv filename 解析；未知时回退 guid） */
      itemGuid: "",
      /** fnos | webrs —— webrs 不回传飞牛 */
      source: "",
      maxPos: 0,
      pos: 0,
      dur: 0,
      paused: false,
      lastReportAt: 0,
      reportedOnce: false,
    };

    /** 清空位置类状态；`all=true` 时连 guid 一起清（播放会话真正结束）。 */
    const reset = (all = false) => {
      s.maxPos = 0;
      s.pos = 0;
      s.dur = 0;
      s.paused = false;
      s.lastReportAt = 0;
      s.reportedOnce = false;
      s.itemGuid = "";
      if (all) {
        s.guid = "";
        s.source = "";
      }
    };

    /** 回传当前位置/时长（force=false 时 10s 最多一次）。 */
    const report = (force = false) => {
      if (!s.guid) return;
      const pos = Math.max(s.maxPos, s.pos);
      if (pos <= 0) return;
      const now = Date.now();
      if (!force && now - s.lastReportAt < PROGRESS_REPORT_MS) return;
      s.lastReportAt = now;
      s.reportedOnce = true;
      void invoke("report_watch_progress", {
        guid: s.guid,
        itemGuid: s.itemGuid || null,
        positionMs: Math.round(pos * 1000),
        durationMs: Math.round(s.dur * 1000),
      }).catch(() => {
        /* 失败不影响播放；下一轮会再报 */
      });
    };

    /** 会话结束：flush 一次 + 回填真实观看时长（<3s 视为误触，不记）。 */
    const finalize = () => {
      const guid = s.guid;
      const pos = Math.max(s.maxPos, s.pos);
      const dur = s.dur;
      report(true);
      reset(true);
      if (!guid || pos <= 3) return;
      const watchedMs = Math.round(pos * 1000);
      const progress = dur > 0 ? Math.min(1, pos / dur) : 0;
      const finished = dur > 0 && pos >= dur * 0.92;
      void invoke("record_watch_session", { guid, watchedMs, progress, finished }).catch(() => {
        /* 回填失败不影响播放 */
      });
    };

    const unlisteners: Array<Promise<() => void>> = [];

    // 定时回传
    const timer = window.setInterval(() => report(false), PROGRESS_REPORT_MS);

    // 页面隐藏 / 应用退出：尽量 flush 最后一次
    const onHide = () => report(true);
    const onVis = () => {
      if (document.visibilityState === "hidden") report(true);
    };
    window.addEventListener("pagehide", onHide);
    window.addEventListener("beforeunload", onHide);
    document.addEventListener("visibilitychange", onVis);

    // 起播 / 切集 / 结束
    unlisteners.push(
      listen<{ phase?: string }>("mpv-load", (e) => {
        const phase = e.payload?.phase;
        if (phase === "start") {
          report(true); // 切集/新起播前结算上一段
          reset(false); // 保留 guid（整季自动切集没有新的 history-recorded）
        } else if (phase === "ended") {
          finalize();
        }
      }),
    );
    unlisteners.push(listen("mpv-end", () => finalize()));

    // 属性变化
    unlisteners.push(
      listen<MpvEvent>("mpv-event", (e) => {
        const p = e.payload;
        if (!p || p.event !== "property-change") return;
        if (p.id === 3) {
          s.paused = p.data === true;
          if (s.paused) report(true); // 暂停即 flush
          return;
        }
        if (p.id === 10) {
          // 整季播放列表自动切到下一集：先结算上一集，再准备本集
          report(true);
          reset(false);
          return;
        }
        if (p.id === 13) {
          if (typeof p.data === "string") {
            const g = parsePlayGuid(p.data);
            if (g) s.itemGuid = g;
          }
          return;
        }
        const d = typeof p.data === "number" ? p.data : NaN;
        if (Number.isNaN(d)) return;
        if (p.id === 1) {
          s.pos = d;
          if (d > s.maxPos) s.maxPos = d;
          // 起播后第一次拿到真实位置：立即上报（继续观看马上出现）。
          // 与旧项目一致：时长未知（duration=0）时不回传飞牛，等 duration 属性到达后再报。
          if (!s.reportedOnce && d > 0 && s.dur > 0) report(true);
        } else if (p.id === 2 && d > 0) {
          s.dur = d;
          // 时长刚到达且已有位置：补一次首报（处理 time-pos 先于 duration 的顺序）。
          if (!s.reportedOnce && s.pos > 0) report(true);
        }
        // id=12 percent-pos 仅作观察，不参与上报（以 time-pos/duration 为准）
      }),
    );

    // 起播时后端落库后广播的本地记录键
    unlisteners.push(
      listen<HistoryRecorded>("history-recorded", (e) => {
        const g = e.payload?.guid;
        if (typeof g === "string" && g) {
          s.guid = g;
          s.source = e.payload?.source ?? "";
          // 点击的就是这一集：先按它上报，mpv filename 事件到来后自动校正（切集）。
          if (!s.itemGuid) s.itemGuid = g;
        }
      }),
    );

    return () => {
      window.clearInterval(timer);
      window.removeEventListener("pagehide", onHide);
      window.removeEventListener("beforeunload", onHide);
      document.removeEventListener("visibilitychange", onVis);
      unlisteners.forEach((p) => void p.then((f) => f()).catch(() => {}));
    };
  }, []);
}
