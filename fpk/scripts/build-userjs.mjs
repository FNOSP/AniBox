// scripts/build-userjs.mjs — 将复用自 AniBox 的 preload 代码打包成浏览器可用的 IIFE 注入脚本。
// 用法：node scripts/build-userjs.mjs
import { build } from 'esbuild';
import { fileURLToPath } from 'url';
import path from 'path';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

// 构建时间：在打包/构建时生成（本地时间，精确到分钟），注入为 __BUILD_TIME__ 常量，
// 供「设置面板 → 关于」页在版本号后展示，如 `v1.3.0 (2026-10-21 20:52)`。
const pad2 = (n) => String(n).padStart(2, '0');
const now = new Date();
const buildTime = `${now.getFullYear()}-${pad2(now.getMonth() + 1)}-${pad2(now.getDate())} ${pad2(now.getHours())}:${pad2(now.getMinutes())}`;
console.log(`build time: ${buildTime}`);

await build({
  entryPoints: [path.join(root, 'src/web-entry.ts')],
  bundle: true,
  format: 'iife',
  outfile: path.join(root, 'dist/fntv-plus.user.js'),
  platform: 'browser',
  target: ['es2019'],
  // 把 `electron` 别名到浏览器垫片，避免打包真实 electron；
  // fs/path（部分插件的本地文件缓存）别名到浏览器垫片安全降级
  alias: {
    electron: path.join(root, 'src/shim/electron.js'),
    fs: path.join(root, 'src/shim/node_fs.js'),
    path: path.join(root, 'src/shim/node_path.js'),
  },
  // 代码中仅用到 process.env.NODE_ENV；
  // banner：在 IIFE 最前挂 window.require/__dirname 兜底（先于所有模块初始化，防顶层 ReferenceError 中断）
  define: { 'process.env.NODE_ENV': '"production"', __BUILD_TIME__: JSON.stringify(buildTime) },
  banner: { js: `try{if(typeof window!=='undefined'){if(typeof window.require==='undefined'){window.require=function(id){if(id==='electron'||id==='electron/main')return (window.__fntvShim||{});throw new Error('网页端不支持 Node 模块: '+id)};}if(typeof window.__dirname==='undefined')window.__dirname='/fntv-web';}}catch(e){}` },
  logLevel: 'info',
  legalComments: 'inline',
  treeShaking: true,
});

console.log('✅ built dist/fntv-plus.user.js');
