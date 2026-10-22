/* [progress] 全局播放进度上报（与旧项目 Fntv-Plus 的「进度回传 + 继续观看」链路对齐）。
 *
 * 为什么必须全局：进度/继续观看不能依赖「观影记录」页挂载 —— 用户正常播放时不会进那个页面。
 * 本 hook 在 App 顶层无条件挂载，只监听 mpv 事件（Rust 侧 mpv.rs 发出）：
 *
 *   事件来源（mpv.rs / player.rs 实际发出）：
 *     · `mpv-load {phase:"start"}`  起播 / 整季播放列表切到下一集（mpv start-file）
 *     · `mpv-load {phase:"loaded"}` 当前条目就绪（mpv file-loaded）
 *     · `mpv-load {phase:"ended"}`  单集结束（end-file，带 reason）/ mpv 进程退出（reason="ipc-closed"）
 *     · `mpv-end`                   播放失败
 *     · `mpv-event` id=1 time-pos / 2 duration / 3 pause / 9 idle-active / 10 playlist-pos /
 *                   11 playlist-count / 13 path（当前集代理 URL；[fix] 原为 filename，basename 解析不出 guid）
 *     · `history-recorded {guid,source}` 起播时后端落库后广播（本地记录键，整季切集不变）
 *
 * 上报节奏：
 *     · 起播后**第一个 time-pos > 0** 立即上报一次（继续观看马上出现）；
 *     · 播放中每 10s 一次；
 *     · 暂停 / 切集 / 单集结束 / mpv 退出 / 页面隐藏 立即 flush；
 *     · 整季真正结束时再用 `record_watch_session` 回填真实观看时长（年度报告用）。
 *
 * [fix] 整季连播切集的关键：mpv 自动切下一集时，`end-file` 与 `start-file` 会连续出现。
 * 过去把 `end-file` 一律当作「本次播放结束」，`finalize()` 把 `guid` 清空 —— 而后续集
 * 不会再有 `history-recorded`，于是第 2 集起 `report()` 因 `guid` 为空全部变成空操作，
 * 「继续观看」永远停在第 1 集。现在：
 *   · `end-file` 只结算当前集并**延迟** finalize；若紧接着有 `start-file`（切集）就取消，
 *     `guid` 保持不变，新一集的 `filename` 事件更新 `itemGuid` 后继续上报；
 *   · 只有真正结束（无下一集 / mpv 退出）才 finalize 并清空会话。
 *
 * 两条链路（后端 `report_watch_progress` 内部）：
 *     ① 本地 history.json 按剧级 series_key 更新（不新增重复记录）；
 *     ② 飞牛 `POST /v/api/v1/play/record` —— 首页「继续观看」的服务端数据源。
 */
import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** 进度上报间隔：5s（写盘 + 飞牛回传节流；其余时机另有立即 flush）。
 *  [fix] 原为 10s —— 进程被强杀时最多丢 10s；缩短到 5s 缩小丢失窗口。 */
const PROGRESS_REPORT_MS = 5_000;

/** [fix] time-pos 单次跳变超过该值视为 seek → 立即 flush 一次。 */
const SEEK_JUMP_SECS = 15;

/**
 * `end-file` 后等待多久判定「本次播放真的结束」。
 * mpv 自动连播 / 手动切集会在 `end-file` 后立刻发 `start-file`（远小于该值）；
 * 只有最后一集播完或用户停止时才会真的等满，再 finalize。
 */
const FINALIZE_DELAY_MS = 2_500;

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

type MpvLoad = {
  phase?: string;
  reason?: string;
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
      /** 当前播放列表位置/总数（-1 / 0 = 未知） */
      playlistPos: -1,
      playlistCount: 0,
    };

    /** 延迟 finalize 的计时器（切集时会被取消）。 */
    let finalizeTimer = 0;
    const cancelFinalize = () => {
      if (finalizeTimer) {
        window.clearTimeout(finalizeTimer);
        finalizeTimer = 0;
      }
    };

    /** 只清位置类状态（切集时用，保留 guid / itemGuid）。 */
    const resetPosition = () => {
      s.maxPos = 0;
      s.pos = 0;
      s.dur = 0;
      s.paused = false;
      s.lastReportAt = 0;
      s.reportedOnce = false;
    };

    /** 清空整个会话（真正结束时用）。 */
    const resetSession = () => {
      cancelFinalize();
      resetPosition();
      s.guid = "";
      s.source = "";
      s.itemGuid = "";
      s.playlistPos = -1;
      s.playlistCount = 0;
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
      cancelFinalize();
      const guid = s.guid;
      const pos = Math.max(s.maxPos, s.pos);
      const dur = s.dur;
      report(true);
      resetSession();
      if (!guid || pos <= 3) return;
      const watchedMs = Math.round(pos * 1000);
      const progress = dur > 0 ? Math.min(1, pos / dur) : 0;
      const finished = dur > 0 && pos >= dur * 0.92;
      void invoke("record_watch_session", { guid, watchedMs, progress, finished }).catch(() => {
        /* 回填失败不影响播放 */
      });
    };

    /** 单集结束：先结算，再延迟 finalize；若下一集马上开始（start）会取消。 */
    const settleEpisode = () => {
      report(true);
      cancelFinalize();
      finalizeTimer = window.setTimeout(() => {
        finalizeTimer = 0;
        finalize();
      }, FINALIZE_DELAY_MS);
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
      listen<MpvLoad>("mpv-load", (e) => {
        const phase = e.payload?.phase;
        if (phase === "start") {
          // 新文件开始加载：取消「上集已结束」的延迟 finalize（这就是切集）。
          cancelFinalize();
          // 结算上一集（若位置尚未清零）；再为新一集清位置。
          report(true);
          resetPosition();
          // 当前集 guid 由随后的 filename 事件给出；先清掉，避免沿用上一集。
          s.itemGuid = "";
        } else if (phase === "loaded") {
          // 下一集已就绪：确认是切集，取消结束判定。
          cancelFinalize();
        } else if (phase === "ended") {
          if (e.payload?.reason === "ipc-closed") {
            // mpv 进程退出：会话真正结束，立即结算。
            finalize();
          } else {
            // 单集结束（eof/stop/quit）：结算后延迟判定是否切集。
            settleEpisode();
          }
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
        if (p.id === 9) {
          // idle-active：无文件加载。若为 true 且不是加载过渡，通常意味着已停止/结束。
          // 不在此 finalize（切集瞬间也可能短暂 idle），交给 ended/ipc-closed 判定。
          return;
        }
        if (p.id === 10) {
          // 播放列表位置变化（切集）：只更新位置计数，**不清 itemGuid / 位置**，
          // 新一集的 guid 由 filename 事件给出。
          if (typeof p.data === "number") s.playlistPos = p.data;
          return;
        }
        if (p.id === 11) {
          if (typeof p.data === "number") s.playlistCount = p.data;
          return;
        }
        if (p.id === 13) {
          if (typeof p.data === "string") {
            const g = parsePlayGuid(p.data);
            if (g && g !== s.itemGuid) {
              s.itemGuid = g;
              // 新一集的 filename 已到：说明切集成功，取消结束判定。
              cancelFinalize();
            }
          }
          return;
        }
        const d = typeof p.data === "number" ? p.data : NaN;
        if (Number.isNaN(d)) return;
        if (p.id === 1) {
          const prevPos = s.pos;
          s.pos = d;
          if (d > s.maxPos) s.maxPos = d;
          // [fix] 关键节点：检测到 seek（位置跳变）立即上报一次，缩小丢失窗口。
          if (s.reportedOnce && Math.abs(d - prevPos) > SEEK_JUMP_SECS) report(true);
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
      cancelFinalize();
      window.removeEventListener("pagehide", onHide);
      window.removeEventListener("beforeunload", onHide);
      document.removeEventListener("visibilitychange", onVis);
      unlisteners.forEach((p) => void p.then((f) => f()).catch(() => {}));
    };
  }, []);
}
