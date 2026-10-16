/* [lc-1640] 顶栏 ⟳「按当前页强制刷新」的统一通道。

   背景：顶栏刷新键原先在 App.tsx 里**写死**成"不管在哪一页都只重拉首页数据"。
   用户要求"在哪个页面点刷新，就强制刷新当前页面的数据"。各页面的刷新入口分散在不同
   组件里（且不少已有自己的 force 通道：TmdbPane 的失败重试、BgmPane 的 ⟳、DetailView 的
   refreshAll、WatchHistoryView 的「立即同步」），所以**不重写取数逻辑**，只搭一条最轻的通道：

     · tick      —— 顶栏每点一次 ⟳ 自增；视图/面板 useEffect 监听它，触发**自己已有的** force 刷新。
     · reportBusy —— 视图/面板上报"本次刷新是否在进行"（id 区分来源）；App 据此汇总顶栏 ⟳ 的 spinning 态。

   为什么用 Context 而不是逐层传 props：刷新键要触达的 5 个目标里，TmdbPane/BgmPane 埋在
   DiscoverView 之下（隔一层），且 App 需要把各视图的 busy 汇总回顶栏 —— Context 让两侧都零 prop 穿透。
   （仓库既有风格允许 hooks：useDelayedUnmount / useLibIndex 同属此目录。） */
import { createContext, useContext } from "react";

/** 视图/面板上报 busy：id = 来源标识（"tmdb"/"bgm"/"lib"/"history"/"detail"），busy = 是否进行中。 */
export type RefreshBusyReporter = (id: string, busy: boolean) => void;

export type RefreshCtx = {
  /** 顶栏 ⟳ 每点一次自增（起始 0）；视图监听它触发自己的 force 刷新（跳过本地 TTL）。 */
  tick: number;
  /** 上报"本次刷新是否在进行"；App 据此汇总顶栏 ⟳ 的 spinning 态。 */
  reportBusy: RefreshBusyReporter;
};

/** 默认值（tick=0、reportBusy 空实现）：没套 Provider 时视图照常工作，只是不会被顶栏 ⟳ 触发。 */
export const RefreshContext = createContext<RefreshCtx>({ tick: 0, reportBusy: () => {} });

export const useRefresh = (): RefreshCtx => useContext(RefreshContext);
