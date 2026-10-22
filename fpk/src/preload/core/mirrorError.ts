// preload/core/mirrorError.ts
// 统一「镜像站点故障」文案（TMDB / Bangumi 数据获取失败）。
//
// 背景：主进程 bridge 返回的错误里可能带镜像 base url（如 https://tmdb.690075.xyz/...）、
// 官方域名，或「HTTP 状态 + URL」拼接串。这些信息只应进日志，绝不能出现在任何页面的
// 错误框 / 详情卡 / 浮层 / 按钮 title 里。
//
// 约定：展示前一律过 friendlyError()。原文含 URL / 镜像域名 / 已知上游域名 / 「镜像」字样
// 时，替换为统一文案；其余错误（如「缺少季号」）保留原文，避免误伤可操作提示。
// 原始错误仍由各调用方写 console / 日志，便于排查。

import { t } from './i18n';

/** 统一文案（中文原文即 i18n key，见 core/i18n.ts）。 */
export const MIRROR_UNAVAILABLE = '当前镜像站点故障，请稍后尝试';

/** URL / 镜像域名 / 上游域名 / 镜像失败特征。 */
const UPSTREAM_ADDRESS_RE =
    /https?:\/\/|690075\.xyz|workers\.dev|themoviedb\.org|tmdb\.org|image\.tmdb\.org|bgm\.tv|镜像不可用|镜像取数失败|镜像站点/i;

/** 取出错误对象里的可展示文本（兼容 string / Error / 任意对象）。 */
function rawText(raw: unknown): string {
    if (typeof raw === 'string') return raw;
    if (raw == null) return '';
    if (typeof raw === 'object') {
        const msg = (raw as { message?: unknown }).message;
        if (typeof msg === 'string') return msg;
    }
    return String(raw);
}

/**
 * 把主进程 / IPC 错误转成可展示文案。
 * @param raw 原始错误（string / Error / { error } / undefined）
 * @param fallback 原文为空时的兜底文案
 */
export function friendlyError(raw: unknown, fallback = ''): string {
    const text = rawText(raw).trim();
    if (!text) return fallback;
    if (UPSTREAM_ADDRESS_RE.test(text)) return t(MIRROR_UNAVAILABLE);
    return text;
}

/** 该错误是否属于「镜像/上游地址」类错误（需要展示统一文案）。 */
export function isMirrorError(raw: unknown): boolean {
    const text = rawText(raw).trim();
    return text !== '' && UPSTREAM_ADDRESS_RE.test(text);
}
