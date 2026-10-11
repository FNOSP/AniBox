/* 飞牛影视真图标 —— 字形逐字节取自 trim 前端资源包（用户 NAS /v/assets 实际分发的 JS）：
   - star / folder：主应用 chunk 具名图标（kebab 名原样保留）
   - home / grid / movie / tv / live：Semi 图标 chunk（旧版路径），按路径形状比对官方 @douyinfe/semi-icons 定名
   与 emoji 相比即飞牛原生观感；统一 24×24 viewBox / currentColor / evenodd。 */

import type { ReactElement } from "react";

type IconProps = { size?: number; className?: string };

function mk(paths: string[]): (p: IconProps) => ReactElement {
  return function Icon({ size = 16, className }: IconProps) {
    return (
      <svg
        width={size}
        height={size}
        viewBox="0 0 24 24"
        xmlns="http://www.w3.org/2000/svg"
        fill="currentColor"
        focusable="false"
        aria-hidden="true"
        className={className}
      >
        {paths.map((d, i) => (
          <path key={i} d={d} fillRule="evenodd" clipRule="evenodd" />
        ))}
      </svg>
    );
  };
}

export const IcoHome = mk([
  "M11.386 1.21c.36-.28.867-.28 1.228 0l9 7c.244.19.386.482.386.79v11a3 3 0 01-3 3H5a3 3 0 01-3-3V9c0-.308.142-.6.386-.79l9-7z",
]);

export const IcoStar = mk([
  "M10.75 1.9c.4-1.2 2.1-1.2 2.5 0l1.99 6.12h6.45a1.3 1.3 0 0 1 .77 2.37l-5.22 3.78 2 6.11c.39 1.2-1 2.2-2.02 1.46L12 17.96l-5.22 3.78c-1.02.75-2.4-.25-2.01-1.46l1.99-6.11-5.22-3.78a1.3 1.3 0 0 1 .77-2.37h6.45l2-6.12Z",
]);

export const IcoGrid = mk([
  "M7 19a2 2 0 114 0 2 2 0 01-4 0zm6 0a2 2 0 114 0 2 2 0 01-4 0zm-6-7a2 2 0 114 0 2 2 0 01-4 0zm6 0a2 2 0 114 0 2 2 0 01-4 0zM7 5a2 2 0 114 0 2 2 0 01-4 0zm6 0a2 2 0 114 0 2 2 0 01-4 0z",
]);

export const IcoMovie = mk([
  "M20 3a3 3 0 013 3v12a3 3 0 01-3 3H4a3 3 0 01-3-3V6a3 3 0 013-3h16zM9 8c-2.208 0-4 1.792-4 4a4.001 4.001 0 006.041 3.44c.474-.282.47-.929.08-1.319-.39-.39-1.029-.356-1.559-.201a2 2 0 11.001-3.84c.53.155 1.17.19 1.56-.201.39-.39.394-1.036-.08-1.318A3.979 3.979 0 009 8zm7 0c-2.208 0-4 1.792-4 4a4.001 4.001 0 006.041 3.44c.474-.282.47-.928.08-1.319-.39-.39-1.028-.357-1.557-.201a2 2 0 110-3.84c.53.155 1.168.19 1.559-.201.39-.39.394-1.036-.08-1.318A3.979 3.979 0 0016 8z",
]);

export const IcoTv = mk([
  "M21 7.5a1 1 0 00-.999-.999H4a1 1 0 00-.999.999v9a1 1 0 00.999 1h16.001a1 1 0 00.999-1v-9zm2.001 9a3 3 0 01-3 3H4a3 3 0 01-3-3v-9a3 3 0 013-3h16.001a3 3 0 013 3v9z",
]);

export const IcoLive = mk([
  "M17 8.759V8a3 3 0 00-3-3H4a3 3 0 00-3 3v8a3 3 0 003 3h10a3 3 0 003-3v-1.132l3.668 2.446a1.502 1.502 0 002.119-.477 1.5 1.5 0 00.213-.77V7.87a1.5 1.5 0 00-2.256-1.296L17 8.76z",
]);

export const IcoFolder = mk([
  "M5 2a3 3 0 0 0-3 3v3h20c0-2-1-4-3-4h-6.45a3 3 0 0 1-1.87-.66l-1.13-.9A2 2 0 0 0 8.3 2H5Z",
  "M22 10H2v9a3 3 0 0 0 3 3h14a3 3 0 0 0 3-3v-9Z",
]);

export const IcoSun = mk([
  "M6.76 4.84l-1.8-1.79-1.41 1.41 1.79 1.79 1.42-1.41zM4 10.5H1v2h3v-2zm9-9.95h-2V3.5h2V.55zm7.45 3.91l-1.41-1.41-1.79 1.79 1.41 1.41 1.79-1.79zm-3.21 13.7l1.79 1.8 1.41-1.41-1.8-1.79-1.4 1.4zM20 10.5v2h3v-2h-3zm-8-5c-3.31 0-6 2.69-6 6s2.69 6 6 6 6-2.69 6-6-2.69-6-6-6zm-1 16.95h2V19.5h-2v2.95zm-7.45-3.91l1.41 1.41 1.79-1.8-1.41-1.41-1.79 1.8z",
]);

export const IcoMoon = mk([
  "M12 3a9 9 0 1 0 9 9c0-.46-.04-.92-.1-1.36a5.389 5.389 0 0 1-4.4 2.26 5.403 5.403 0 0 1-3.14-9.8c-.44-.06-.9-.1-1.36-.1z",
]);