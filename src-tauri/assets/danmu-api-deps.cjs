/**
 * [lc-1561] 自建弹幕接口（danmu_api）**原样移植**的支持桩。
 *
 * 主仓库 `src/main/common/danmuApi.ts`（1526 行）是 Electron 主进程模块，依赖四样环境：
 *   electron.app.getPath('userData') / logger / fn_config / proxyAgent。
 * 本文件只提供这四样的等价物，**不含任何弹幕业务逻辑** —— danmuApi 本体转译后逐字保留
 * （见 assets/danmu-api.cjs，仅 4 行 require 改写指向这里），保证与原项目行为一致。
 *
 * 配置来源：Rust 侧写出的 `%TEMP%/anibox-danmu-config.json`（每次读取都重新读盘 ⇒
 * 用户在设置面板改完立即生效，不需要重启应用或重启 shim —— 与原项目 fnConfig 行为一致）。
 */
'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');

const CONF_PATH = path.join(os.tmpdir(), 'anibox-danmu-config.json');

function readConf() {
    try {
        return JSON.parse(fs.readFileSync(CONF_PATH, 'utf8')) || {};
    } catch (_) {
        return {};
    }
}

const cacheDir = path.join(os.tmpdir(), 'anibox-danmu-mem');
try { fs.mkdirSync(cacheDir, { recursive: true }); } catch (_) { /* 已存在 */ }

/** electron.app 的最小替身：只用到 getPath('userData')（放系列记忆 json） */
const app = {
    getPath(name) {
        return name === 'userData' ? cacheDir : cacheDir;
    },
};

/** logger 替身：输出走 stderr（Rust 侧把 shim 的 stderr 落 anibox-danmaku-shim.log） */
function makeLogger(name) {
    const emit = (level) => (...args) => {
        const line = args.map((a) => (typeof a === 'string' ? a : JSON.stringify(a))).join(' ');
        process.stderr.write(`[${name}][${level}] ${line}\n`);
    };
    return { info: emit('info'), warn: emit('warn'), error: emit('error'), debug: emit('debug') };
}
const logger = { component: (n) => makeLogger(n || 'danmuApi') };

/** fn_config 替身：读 Rust 写出的配置（多地址去空/去尾斜杠/去重，口径同原项目） */
function cleanBase(b) {
    return String(b || '').trim().replace(/\/+$/, '');
}
const fnConfig = {
    getDanmuApiEnabled() {
        return readConf().enabled === true;
    },
    getDanmuApiBase() {
        const c = readConf();
        return typeof c.base === 'string' ? cleanBase(c.base) : '';
    },
    getDanmuApiBases() {
        const c = readConf();
        const list = Array.isArray(c.bases) ? c.bases.map(cleanBase).filter(Boolean) : [];
        if (list.length) return [...new Set(list)];
        const single = typeof c.base === 'string' ? cleanBase(c.base) : '';
        return single ? [single] : [];
    },
    // 弹弹play 自定义凭证（设置面板；默认空 ⇒ 用内置密文凭证，与原项目一致）
    getDandanplayAppId() {
        return String(readConf().ddpAppId || '');
    },
    getDandanplayAppSecret() {
        return String(readConf().ddpAppSecret || '');
    },
};

/** 飞牛播放元信息（Rust 在每次 play_item 时写出）：系列记忆键 + 弹弹play 兜底所需 */
const PLAYMETA_PATH = path.join(os.tmpdir(), 'anibox-play-meta.json');
function readPlayMeta() {
    try {
        return JSON.parse(fs.readFileSync(PLAYMETA_PATH, 'utf8')) || {};
    } catch (_) {
        return {};
    }
}
fnConfig.readConfig = () => {
    const c = readConf();
    const m = readPlayMeta();
    return {
        domain: String(c.server || ''),
        token: String(c.token || ''),
        // [lc-1254] 季 guid 直供：Rust 解析 play/info 后已知道 parent_guid，写进 play-meta
        // ⇒ 不需要像主仓库那样再发一次 HTTP 去解析（原项目 resolveSeriesKey 的等价结果）
        parentGuid: String(m.parentGuid || ''),
        episodeGuid: String(m.episodeGuid || ''),
    };
};

/**
 * proxyAgent 替身：复用环境变量代理（HTTPS_PROXY/http_proxy/socks*）。
 * 自建源通常部署在**内网/Tailscale**（如 192.168.x.x、100.66.x.x），本就不该走梯子；
 * 原项目这里也只是「诊断时的可选旁路测试」。返回 null = 直连，与未配置代理时一致。
 */
function resolveProxyAgent() {
    return null;
}

/* ── 脚本目录通道：shim 启动时已确知（由 mpv 路径推导并校验过 bili_danmaku.js 存在） ── */
let scriptDir = '';
function setScriptDir(dir) {
    scriptDir = String(dir || '');
}
function getScriptDir() {
    return scriptDir;
}

/**
 * 弹弹play 源（MPV 链路）的替身：**本模块不实现**。
 *
 * 主仓库 `src/main/common/dandanplay.ts` 内嵌**第三方服务的凭证密文常量**（`enc:v1:` +
 * 根密钥掩码），按本仓库入库规则「凭据绝不入库」不移植。
 *
 * 这不影响 MPV 用户可见行为：MPV 的弹弹play 兜底由 **uosc_danmaku 的 Lua 侧**承担
 * （`apis/dandanplay.lua`：自己解析 episodeId → 内置凭证降级挂起 /comment → 自建源/B站
 * 都未命中才发请求；实测日志「弹弹play：内置凭证降级——暂缓 /comment」→ 其他源成功则
 * 「取消内置凭证 /comment（省一次请求）」）。弹弹play 官方规约本也要求「按需使用」，
 * 两处各发一次是重复请求，故应用侧不重复实现。
 *
 * 接口形状与原模块一致，但一律回「未在应用侧启用」——让 traces 如实说明，不假装成功。
 */
const dandanplayLuaOwned = {
    ID_PREFIX: 'ddp:',
    /** 不参与应用侧候选合并（Lua 候选菜单有自己的「番剧库」段） */
    async candidates() {
        return [];
    },
    async autoFetchBili() {
        return {
            ok: false,
            error: '弹弹play 兜底由 MPV 弹幕脚本按需触发（应用侧不含第三方凭证，故不重复请求）',
        };
    },
    async fetchById() {
        return {
            ok: false,
            error: '弹弹play 条目直取需在播放器内操作（应用侧不含第三方凭证）',
        };
    },
};

module.exports = {
    app,
    logger,
    fnConfig,
    resolveProxyAgent,
    setScriptDir,
    getScriptDir,
    dandanplayLuaOwned,
};
