import { ipcRenderer } from 'electron';

// embyWall/modals/feedback.ts — 外链入口：「加入Q群」（原反馈弹窗组已移除）
// 由 scripts/embywall-split.js 从 embyWall.ts 整段抽取；改实现请改这里，不要在入口文件里补。

// [v1.8.0] Web 版(fpk)仓库——关于页的 GitHub 入口（桌面版仓库在关于页描述中另附）
export const ABOUT_LINK_URL = 'https://github.com/FNOSP/AniBox';

// [lc-361] QQ 交流群加群链接（qm.qq.com 为网页兜底，桌面端/移动端会拉起 QQ 加群页）
// 来源：仓库既有常量（与 D:\GitHub\Fntv-Plus 同值），非占位。
export const QQ_GROUP_URL = 'https://qm.qq.com/q/dUnIQVvoIw';

/**
 * 统一外链打开通道：
 *  - fpk/桌面端：app:open-external → 主进程 shell.openExternal
 *  - 网页端：shim 的 app:open-external → window.open 新标签
 *  - invoke 异常时降级 window.open，保证任何环境都能打开
 */
export const openExternalUrl = (url: string): void => {
  try {
    ipcRenderer.invoke('app:open-external', url).catch(() => {
      try { window.open(url, '_blank', 'noopener'); } catch { /* ignore */ }
    });
  } catch {
    try { window.open(url, '_blank', 'noopener'); } catch { /* ignore */ }
  }
};

/** 「加入Q群」：点击直接打开加群链接（不再弹反馈选择/问卷/日志上传弹窗） */
export const openQQGroup = (): void => openExternalUrl(QQ_GROUP_URL);
