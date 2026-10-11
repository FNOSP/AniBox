import { app } from 'electron';
import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';
import * as crypto from 'crypto';
import * as logger from '../../../modules/logger';
import * as fnConfig from '../../../modules/fn_config/config';

/**
 * MPV配置文件管理插件
 * 定时检查用户配置目录中的scripts文件夹，不存在则从应用中复制
 */

let configCheckInterval: NodeJS.Timeout | null = null;

// 获取用户MPV配置目录
function getMpvConfigDir(): string {
    const homeDir = os.homedir();
    if (process.platform === 'win32') {
        return path.join(homeDir, 'AppData', 'Roaming', 'mpv');
    } else {
        return path.join(homeDir, '.config', 'mpv');
    }
}

// 获取应用中的portable_config目录
// ⚠️ 关键：必须返回 MPV 实际读取的【可写】目录。
//   - 开发模式：app.getAppPath() = 项目根目录，CWD = 项目根目录 → 两者一致，可写。
//   - 打包模式：app.getAppPath() = resources/app.asar（只读虚拟路径！），
//     而 extraFiles 把 third_party/fntv-mpv 解压到【exe 同级目录】（可写），
//     MPV 以相对路径 + CWD=exe目录 启动 → 实际读取 exe 目录下的 portable_config。
//     故打包后必须用 path.dirname(app.getPath('exe')) 拼接，否则写 mpv-user.conf 会写进
//     只读 asar 导致静默失败（面板改了默认着色器但不生效）。
function getPortableConfigDir(): string {
    const sub = path.join('third_party', 'fntv-mpv', 'portable_config');
    if (process.platform === 'darwin') {
        // macOS: third_party目录在应用包的Contents目录下（extraFiles 解压到 Contents/）
        const appPath = app.getAppPath();
        const contentsPath = path.dirname(path.dirname(appPath)); // 从app.asar向上两级到Contents
        return path.join(contentsPath, 'third_party', 'fntv-mpv', 'portable_config');
    } else if (process.platform === 'win32') {
        // Windows: extraFiles 将 third_party/fntv-mpv 解压到 exe 同级目录
        if (app.isPackaged) {
            return path.join(path.dirname(app.getPath('exe')), sub);
        }
        return path.join(app.getAppPath(), sub);
    } else {
        // Linux: 同 Windows，extraFiles 解压到 exe 同级目录
        if (app.isPackaged) {
            return path.join(path.dirname(app.getPath('exe')), sub);
        }
        return path.join(app.getAppPath(), sub);
    }
}

// 递归复制目录
function copyDirectoryRecursive(source: string, destination: string): void {
    if (!fs.existsSync(source)) {
        logger.log(`Source directory does not exist: ${source}`);
        return;
    }

    // 创建目标目录
    if (!fs.existsSync(destination)) {
        fs.mkdirSync(destination, { recursive: true });
    }

    const items = fs.readdirSync(source);

    items.forEach(item => {
        const sourcePath = path.join(source, item);
        const destPath = path.join(destination, item);

        const stat = fs.statSync(sourcePath);

        if (stat.isDirectory()) {
            copyDirectoryRecursive(sourcePath, destPath);
        } else {
            fs.copyFileSync(sourcePath, destPath);
            logger.log(`Copied: ${sourcePath} -> ${destPath}`);
        }
    });
}

// 检查并复制配置文件
function checkAndCopyMpvConfig(): void {
    try {
        const mpvConfigDir = getMpvConfigDir();
        const scriptsDir = path.join(mpvConfigDir, 'scripts');
        const portableConfigDir = getPortableConfigDir();

        // 检查scripts目录是否存在
        if (!fs.existsSync(scriptsDir)) {
            logger.log(`Scripts directory not found, copying from portable config...`);

            // 确保MPV配置目录存在
            if (!fs.existsSync(mpvConfigDir)) {
                fs.mkdirSync(mpvConfigDir, { recursive: true });
                logger.log(`Created MPV config directory: ${mpvConfigDir}`);
            }

            // 复制portable_config目录中的所有内容到用户配置目录
            if (fs.existsSync(portableConfigDir)) {
                copyDirectoryRecursive(portableConfigDir, mpvConfigDir);
                logger.log(`MPV configuration copied successfully from ${portableConfigDir} to ${mpvConfigDir}`);
            } else {
                logger.log(`Portable config directory not found: ${portableConfigDir}`);
            }
        } else {
            logger.debug(`Scripts directory already exists: ${scriptsDir}`);
        }
    } catch (error) {
        logger.error('Error in checkAndCopyMpvConfig:', error);
    }
}

// 启动定时检查
function startConfigCheck(): void {
    // 立即执行一次检查
    checkAndCopyMpvConfig();
    // [lc-486] 升级兼容：确保新增的插帧脚本/配置同步到用户 MPV 目录（老用户 scripts 目录已存在，首次拷贝不会覆盖）
    syncNewInterpFiles();

    // 设置定时检查（每分钟检查一次）
    configCheckInterval = setInterval(() => {
        checkAndCopyMpvConfig();
        syncNewInterpFiles();
    }, 60 * 1000);

    logger.info('MPV config check started, checking every 1 minute');
}

// [lc-486] 升级兼容：把新增的插帧文件（Lua 脚本 + conf）从 portable_config 同步到用户 MPV 目录。
// 仅当目标缺失时才拷贝，绝不覆盖用户已有的自定义内容。
function syncNewInterpFiles(): void {
    try {
        const srcDir = getPortableConfigDir();
        const dstDir = getMpvConfigDir();
        // fntv_interp.lua / fntv_mediainfo.lua / fntv_format_badge.lua 为应用全量托管（用户不应手改），
        // 始终覆盖以确保老用户拿到最新逻辑（如新增 nvidia 引擎档、起播格式标签、徽章错位协调）；
        // 各 script-opts conf 由应用侧或脚本默认值负责，此处仅做缺失补位，不覆盖用户已生成的配置。
        // [lc-1354] 字幕两件套同样全量托管：fntv_nas_sub.lua（NAS/在线字幕菜单，随版本迭代）
        //   与 sub-assrt.lua（第三方射手网脚本原样捆绑）；token 配置只做缺失补位。
        const always = ['scripts/fntv_interp.lua', 'scripts/fntv_mediainfo.lua', 'scripts/fntv_format_badge.lua', 'scripts/fntv_aq.lua',
            'scripts/fntv_menu.lua', 'scripts/fntv_nas_sub.lua', 'scripts/sub-assrt.lua',
            // [lc-1355] 功能补齐（对照 mpv-Yaozhi 杳知菜单）：第三方脚本应用全量托管（原样捆绑不改动）
            'scripts/persist_properties.lua', 'scripts/seek-feedback.lua', 'scripts/autoload.lua',
            'scripts/blacklist-extensions.lua', 'scripts/auto-save-state.lua', 'scripts/trackselect.lua',
            'scripts/undoredo.lua', 'scripts/alist_speed_indicator.lua', 'scripts/sub-fonts-dir-auto.lua',
            'scripts/fix-avsync.lua', 'scripts/uosc-subtitle-lines.lua', 'scripts/image-subs-brightness.lua',
            'scripts/chapterskip.lua', 'scripts/evafast.lua', 'scripts/dynamic-crop.lua', 'scripts/pip.lua',
            'scripts/mpv-cropscreen.lua', 'scripts/sub_export.lua', 'scripts/keybinding-manager.lua',
            'scripts/recentmenu.lua', 'scripts/autosubsync/autosubsync.lua', 'scripts/autosubsync/main.lua',
            'scripts/autosubsync/menu.lua', 'scripts/autosubsync/subtitle.lua'];
        const missingOnly = ['script-opts/fntv_interp.conf', 'script-opts/fntv_format_badge.conf', 'script-opts/fntv_aq.conf',
            'script-opts/sub_assrt.conf',
            // [lc-1355] 移植脚本的默认配置只做缺失补位，不覆盖用户已生成的配置
            'script-opts/persist_properties.conf', 'script-opts/autoload.conf', 'script-opts/blacklist_extensions.conf',
            'script-opts/auto_save_state.conf', 'script-opts/trackselect.conf', 'script-opts/chapterskip.conf',
            'script-opts/evafast.conf', 'script-opts/dynamic_crop.conf', 'script-opts/pip.conf',
            'script-opts/mpv_cropscreen.conf', 'script-opts/sub_export.conf', 'script-opts/image_subs_brightness.conf',
            'script-opts/recentmenu.conf', 'script-opts/alist_speed_indicator.conf'];
        const copy = (rel: string, overwrite: boolean) => {
            const src = path.join(srcDir, rel);
            const dst = path.join(dstDir, rel);
            if (!fs.existsSync(src)) return;
            if (!overwrite && fs.existsSync(dst)) return;
            const dstParent = path.dirname(dst);
            if (!fs.existsSync(dstParent)) fs.mkdirSync(dstParent, { recursive: true });
            fs.copyFileSync(src, dst);
            logger.info(`[插帧] 已同步文件到用户 MPV 目录: ${dst}${overwrite ? ' (覆盖)' : ''}`);
        };
        always.forEach((rel) => copy(rel, true));
        missingOnly.forEach((rel) => copy(rel, false));
        // [lc-1353] 起播格式徽章位图资产（697 个文件）：以 runtime/manifest.json 为存在标记，
        // 整目录缺失时一次性补拷（已存在则零开销，不逐个 stat）。
        try {
            const srcAssets = path.join(srcDir, 'script-assets', 'startup-format-logos');
            const dstAssets = path.join(dstDir, 'script-assets', 'startup-format-logos');
            if (fs.existsSync(path.join(srcAssets, 'runtime', 'manifest.json'))
                && !fs.existsSync(path.join(dstAssets, 'runtime', 'manifest.json'))) {
                copyDirectoryRecursive(srcAssets, dstAssets);
                logger.info(`[徽章] 已同步位图资产到用户 MPV 目录: ${dstAssets}`);
            }
        } catch (e) {
            logger.error('[徽章] 同步位图资产失败:', e);
        }
        // [lc-490] 启动时按当前引擎同步 N 卡上下文配置（防止设置改过但 mpv-interp-nvidia.conf 状态不一致）
        writeInterpNvidiaContext(fnConfig.getMpvInterpEngine() === 'nvidia');
        // [lc-487] 控制栏「插帧」按钮升级为高亮 cycle: 版本
        syncUoscControls();
    } catch (e) {
        logger.error('[插帧] 同步新增文件失败:', e);
    }
}

// [lc-487] 升级兼容：把用户 uosc.conf 控制栏的「插帧」按钮从旧 command: 标记更新为高亮 cycle: 标记；
// 若用户 uosc.conf 完全没有该按钮，则在 controls= 行末尾插入。只改动我们托管的那一段，不触碰其余自定义。
const INTERP_BTN_NEW = 'cycle:auto_awesome:fntv_interp@fntv_interp:no/yes!?插帧';
const INTERP_BTN_OLD = 'command:auto_awesome:script-message fntv-interp toggle?插帧';

function syncUoscControls(): void {
    try {
        const dst = path.join(getMpvConfigDir(), 'script-opts', 'uosc.conf');
        if (!fs.existsSync(dst)) return; // 用户无此文件则交由 MPV 读 portable_config 内置版本
        let text = fs.readFileSync(dst, 'utf-8');
        if (text.includes(INTERP_BTN_NEW)) return; // 已是最新高亮版
        if (text.includes(INTERP_BTN_OLD)) {
            text = text.replace(INTERP_BTN_OLD, INTERP_BTN_NEW);
        } else {
            // 没有该按钮：在生效的 controls= 行（非注释）末尾的 ,fullscreen 前插入
            text = text.replace(/^controls=([^\n]*),fullscreen/m, 'controls=$1,' + INTERP_BTN_NEW + ',fullscreen');
        }
        fs.writeFileSync(dst, text, 'utf-8');
        logger.info('[插帧] 已更新用户 uosc.conf 控制栏插帧按钮（高亮版）');
    } catch (e) {
        logger.error('[插帧] 更新 uosc.conf 失败:', e);
    }
}

// ─────────────────────────────────────────────────────────────────────────
// [lc-1351] 「超分与补帧」自适应画质（参考 mpv-Yaozhi adaptive-quality / ai-interpolation 安全模型）
//   统一管理空间增强(超分)与时间平滑(补帧)：两项独立开关、默认关闭、auto 档按
//   显卡档位 + 片源分辨率/帧率 + 输出负载 + 画面格式 决策；安全边界（不先降采样再补帧、
//   HDR/DV/HLG 保护、实时过载回退）由 fntv_aq.lua 在播放器内执行，此处只落盘配置。
// ─────────────────────────────────────────────────────────────────────────

// GPU 档位缓存：一次探测终身复用（显卡不会热更换；个别外接场景重启应用即可刷新）
let cachedGpuTier: string | null = null;
let gpuTierPromise: Promise<string> | null = null;

/**
 * 按显卡名称映射档位（对齐 mpv-Yaozhi 的 low/balanced/medium/high/upper_high 五档）
 * upper_high = RTX 4080/4090/5080/5090（4K 高刷预算内）；high = 近代独显；medium = GTX10/16 系与 RTX20 系；
 * low = 核显/老集显；balanced = 兜底（老独显/未知）。
 */
function tierFromGpuName(name: string): string {
    const n = String(name || '').toLowerCase();
    if (/rtx\s*(4090|4080|5090|5080)/.test(n)) return 'upper_high';
    if (/rtx\s*(20[5-9]0)\b/.test(n) || /gtx\s*10[5-9]0/.test(n) || /gtx\s*16[5-9]0/.test(n)) return 'medium';
    if (/arc\s*a?\d/.test(n)) return 'medium';
    if (/rtx\s*\d{4}/.test(n)) return 'high';
    if (/rx\s*[6-9]\d{3}/.test(n)) return 'high';
    if (/rx\s*[4-5]\d{3}/.test(n)) return 'balanced';
    if (/gtx\s*\d{3,4}/.test(n)) return 'medium';
    if (/intel|iris|uhd|hd graphics|graphics 6[0-9]0|amd radeon\(tm\) (r[2-7]|graphics|6[0-9]{2}m)/.test(n)) return 'low';
    return 'balanced';
}

/**
 * 探测 GPU 档位：用 Electron 自带 GPU 信息（零额外进程，无 PowerShell/wmic 依赖）。
 * 多显卡时取独立显卡（vendor 0x10DE/0x1002 优先，Intel iGPU 0x8086 靠后）中档位最高的那块。
 * 失败返回 'auto'（fntv_aq.lua 侧按 balanced 兜底，不会误开重负载）。
 */
export function detectGpuTier(): Promise<string> {
    if (cachedGpuTier) return Promise.resolve(cachedGpuTier);
    if (gpuTierPromise) return gpuTierPromise;
    gpuTierPromise = (async () => {
        let tier = 'auto';
        try {
            const info: any = await app.getGPUInfo('complete');
            const devs: any[] = Array.isArray(info && info.gpuDevice) ? info.gpuDevice : [];
            let best = '';
            const rank: Record<string, number> = { upper_high: 5, high: 4, medium: 3, balanced: 2, low: 1, auto: 0 };
            for (const d of devs) {
                const name = String(d && (d.deviceString || d.device || '') || '');
                if (!name) continue;
                const t = tierFromGpuName(name);
                // vendor 判定独显优先级：NVIDIA(0x10DE=4318) / AMD(0x1002=4098) > Intel(0x8086)
                const vid = Number(d && d.vendorId) || 0;
                const discrete = vid === 0x10de || vid === 0x1002 ? 1 : 0;
                const score = (rank[t] || 0) + discrete * 10;
                const bestScore = (rank[best] || 0) + (best ? 10 : 0);
                if (score > bestScore) best = t;
            }
            if (best && rank[best]) tier = best;
            logger.info(`[超分与补帧] GPU 档位探测: tier=${tier} (devices=${devs.map((d: any) => d.deviceString || d.device || '?').join(' | ') || '无'})`);
        } catch (e) {
            logger.warn('[超分与补帧] GPU 档位探测失败，回退 auto:', e);
        }
        cachedGpuTier = tier;
        return tier;
    })();
    return gpuTierPromise;
}

/**
 * 写入「超分与补帧」配置到 script-opts/fntv_aq.conf（供 fntv_aq.lua 读取）。
 * ⚠️ 双写 portable_config 与用户 MPV 目录（与 writeMpvUserConfig 的 lc-094 策略一致）。
 * @param tier 显卡档位（detectGpuTier 结果；'auto' 时 lua 侧按 balanced 兜底）
 */
export function writeAqConfig(tier?: string): void {
    try {
        const sr = fnConfig.getMpvSuperresMode();
        const sm = fnConfig.getMpvSmoothMode();
        const backend = fnConfig.getMpvRifeBackend() === 'tensorrt' ? 'tensorrt' : 'vulkan';
        const protect4k = fnConfig.getMpvAqProtect4k() !== false;
        const lines = [
            '# Fntv-Plus 「超分与补帧」自适应画质配置',
            '# 由应用「设置面板 > 超分与补帧」写入，请勿手动编辑（修改会被覆盖）。',
            '# 安全模型对齐 mpv-Yaozhi adaptive-quality：两项独立、默认关闭；',
            '# 自动档在 fntv_aq.lua 内按 显卡档位/片源分辨率/帧率/输出负载/画面格式 决策并安全回退。',
            '',
            '# 空间增强（超分）：off / auto / performance / quality',
            'superres_mode=' + sr,
            '# 时间平滑（补帧）：off / auto / performance / quality',
            'smooth_mode=' + sm,
            '',
            '# RIFE 后端：vulkan(随包/基础) / tensorrt(需完整扩展包, 仅兼容 NVIDIA RTX)',
            'rife_backend=' + backend,
            '# 显卡档位：low / balanced / medium / high / upper_high / auto（auto=lua 按 balanced 兜底）',
            'tier=' + (tier || 'auto'),
            '# 4K 补帧性能保护（默认开）：4K 片源不进入重负载方案，过载直接回退不降采样',
            'protect_4k_smooth=' + (protect4k ? 'yes' : 'no'),
            '# 通用色度增强（KrigBilateral，仅在像素预算内自动加载）',
            'auto_chroma=yes',
            '# 智能去色带（仅 8bit/非 HDR/低码率密度片源）',
            'smart_deband=yes',
            ''
        ];
        const content = lines.join('\n');
        const dirs = [getPortableConfigDir(), getMpvConfigDir()];
        for (const dir of dirs) {
            try {
                const scriptOptsDir = path.join(dir, 'script-opts');
                if (!fs.existsSync(scriptOptsDir)) fs.mkdirSync(scriptOptsDir, { recursive: true });
                const target = path.join(scriptOptsDir, 'fntv_aq.conf');
                fs.writeFileSync(target, content, 'utf-8');
                logger.info(`[超分与补帧] 配置已写入: ${target} (sr=${sr}, sm=${sm}, backend=${backend}, tier=${tier || 'auto'}, 4k保护=${protect4k})`);
            } catch (e) {
                logger.error(`[超分与补帧] 写入 fntv_aq.conf 失败: ${dir}`, e);
            }
        }
    } catch (error) {
        logger.error('[超分与补帧] 写入 fntv_aq.conf 失败:', error);
    }
}

/** 应用侧「超分与补帧」设置变更：持久化 → 探测档位 → 重写 conf（档位探测异步，完成后补写 tier）。 */
export function applyAqSettings(sr: string, sm: string, backend: string, protect4k: boolean): void {
    fnConfig.setMpvSuperresMode(sr);
    fnConfig.setMpvSmoothMode(sm);
    fnConfig.setMpvRifeBackend(backend);
    fnConfig.setMpvAqProtect4k(protect4k);
    writeAqConfig(cachedGpuTier || 'auto');
    void detectGpuTier().then((tier) => {
        // 档位探测（首跑异步）完成后用真实档位覆写一次；缓存命中时 tier 相同，写盘幂等
        writeAqConfig(tier);
    });
}

/** [lc-1351→lc-1353] 确保 input.conf 含「Fntv-Plus」功能列表绑定（右键/更多菜单，静态平铺无二级入口）。
 *  存量升级均幂等：① 无绑定 → 追加整块；② 旧「超分与补帧」单卡绑定 / 旧「功能菜单…」二级入口 →
 *  删除旧行后追加整块。功能列表本身来自随包 input.conf（超分与补帧仍打开其模式子菜单）。 */
export function ensureAqMenuBinding(): void {
    const NEW_MARKER = 'fntv-menu-chapters';
    const BLOCK = [
        '_          script-message fntv-aq-menu                                                       #menu: Fntv-Plus > 超分与补帧…',
        '_          script-message fntv_format_badge-toggle                                           #menu: Fntv-Plus > 起播格式标签',
        '_          script-message-to uosc codex-open-music-mode-menu                                 #menu: Fntv-Plus > 音乐模式…',
        '_          script-message-to ontop_mode cycle                                                #menu: Fntv-Plus > 置顶模式',
        '_          script-message fntv-menu-chapters                                                 #menu: Fntv-Plus > 视频章节显示',
        '_          script-message fntv-menu-fullscreen                                               #menu: Fntv-Plus > 起播自动全屏',
        '_          script-message fntv-menu-minimize-pause                                           #menu: Fntv-Plus > 最小化暂停播放',
        '_          script-message fntv-menu-audio-pt                                                 #menu: Fntv-Plus > 音频源码直通',
        '_          script-message-to uosc codex-open-peak-menu                                       #menu: Fntv-Plus > HDR 输出与峰值亮度…',
        '_          script-message-to uosc codex-open-black-level-menu                                #menu: Fntv-Plus > 黑位与输出范围…',
        '_          script-message-to uosc codex-open-subtitle-color-menu                             #menu: Fntv-Plus > 字幕色彩模式…',
        '_          script-message-to uosc codex-open-dock-animation-menu                             #menu: Fntv-Plus > 底栏动画…',
    ].join('\n');
    const dirs = [getPortableConfigDir(), getMpvConfigDir()];
    for (const dir of dirs) {
        try {
            const target = path.join(dir, 'input.conf');
            const existing = fs.existsSync(target) ? fs.readFileSync(target, 'utf-8') : '';
            if (existing.indexOf(NEW_MARKER) >= 0) continue; // 已是平铺功能列表版
            // 移除历史两代单入口行（超分与补帧… / 功能菜单…），再追加整块
            const stripped = existing
                .replace(/^.*script-binding fntv_aq\/menu.*\r?\n?/m, '')
                .replace(/^.*script-binding fntv_menu\/menu.*\r?\n?/m, '');
            fs.writeFileSync(target, stripped.replace(/\s*$/, '') + '\n' + BLOCK + '\n', 'utf-8');
            logger.info('已补写「Fntv-Plus 功能列表」入口(input.conf): ' + target);
        } catch (e: any) {
            logger.warn('补写功能列表绑定失败 (' + dir + '):', e && e.message);
        }
    }
}

// 停止定时检查
function stopConfigCheck(): void {
    if (configCheckInterval) {
        clearInterval(configCheckInterval);
        configCheckInterval = null;
        logger.info('MPV config check stopped');
    }
}

/**
 * MPV 默认着色器预设映射表（key → glsl-shaders 文件名列表）
 * 与 input.conf 中 Ctrl+1~9 的临时切换列表保持一致。
 */
const MPV_SHADER_PRESETS: Record<string, string[]> = {
    off: [],
    // 模式A：大多数1080p动画
    a: [
        'Anime4K_Clamp_Highlights.glsl',
        'Anime4K_Restore_CNN_M.glsl',
        'Anime4K_Upscale_CNN_x2_M.glsl',
        'Anime4K_AutoDownscalePre_x2.glsl',
        'Anime4K_Upscale_CNN_x2_M.glsl'
    ],
    // 模式B：大多数720p动画
    b: [
        'Anime4K_Clamp_Highlights.glsl',
        'Anime4K_Restore_CNN_Soft_M.glsl',
        'Anime4K_Upscale_CNN_x2_M.glsl',
        'Anime4K_AutoDownscalePre_x2.glsl',
        'Anime4K_Upscale_CNN_x2_M.glsl'
    ],
    // 模式A+A：高质量1080p
    aa: [
        'Anime4K_Clamp_Highlights.glsl',
        'Anime4K_Restore_CNN_L.glsl',
        'Anime4K_Upscale_CNN_x2_M.glsl',
        'Anime4K_AutoDownscalePre_x2.glsl',
        'Anime4K_Restore_CNN_L.glsl',
        'Anime4K_Upscale_CNN_x2_M.glsl'
    ],
    // 模式B+B：高质量720p
    bb: [
        'Anime4K_Clamp_Highlights.glsl',
        'Anime4K_Restore_CNN_Soft_L.glsl',
        'Anime4K_Upscale_CNN_x2_M.glsl',
        'Anime4K_AutoDownscalePre_x2.glsl',
        'Anime4K_Restore_CNN_Soft_L.glsl',
        'Anime4K_Upscale_CNN_x2_M.glsl'
    ],
    // 轻量模式：低配置设备
    lite: [
        'Anime4K_Restore_CNN_S.glsl',
        'Anime4K_Upscale_CNN_x2_S.glsl',
        'Anime4K_AutoDownscalePre_x2.glsl',
        'Anime4K_Upscale_CNN_x2_S.glsl'
    ],
    // 仅降噪
    denoise: [
        'Anime4K_Denoise_Bilateral_Mode.glsl'
    ],
    // 真实系：真人/纪录片
    real: [
        'CAS.glsl',
        'ColorVibrance.glsl'
    ],
    // 电影感：动画 + 胶片粒度 + 色彩
    cinema: [
        'Anime4K_Clamp_Highlights.glsl',
        'Anime4K_Restore_CNN_M.glsl',
        'Anime4K_Upscale_CNN_x2_M.glsl',
        'Anime4K_AutoDownscalePre_x2.glsl',
        'Anime4K_Upscale_CNN_x2_M.glsl',
        'FilmGrain.glsl',
        'ColorVibrance.glsl'
    ],
    // 全增强：极致画质
    ultra: [
        'Anime4K_Clamp_Highlights.glsl',
        'Anime4K_Restore_CNN_L.glsl',
        'Anime4K_Upscale_CNN_x2_M.glsl',
        'Anime4K_AutoDownscalePre_x2.glsl',
        'Anime4K_Restore_CNN_L.glsl',
        'Anime4K_Upscale_CNN_x2_M.glsl',
        'CAS.glsl',
        'FilmGrain.glsl',
        'ColorVibrance.glsl'
    ]
};

/**
/** [lc-1069] MPV 渲染预设：三档画质方案（写入 mpv-user.conf 的托管块，随启动重放自动生效）
 *  perf=性能优先(低端机/核显流畅, 仍用 legacy vo=gpu 更轻) · balanced=均衡(默认, gpu-next) · quality=高画质(gpu-next+高质量缩放)
 *  [lc-1075] 三档均配合 mpv.conf 的 hwdec=auto（原生呈现，避免 auto-copy 回拷白屏）；vo=gpu-next 与 hwdec=auto 原生硬件帧呈现最契合。 */
const MPV_RENDER_PRESETS: Record<string, string[]> = {
    perf: [
        'vo=gpu',
        'scale=bilinear',
        'dscale=bilinear',
        'cscale=bilinear',
        'deband=no',
    ],
    balanced: [
        'vo=gpu-next',
        'scale=spline36',
        'dscale=mitchell',
        'cscale=spline36',
        'deband=yes',
    ],
    quality: [
        'vo=gpu-next',
        'scale=ewa_lanczossharp',
        'dscale=hermite',
        // [lc-1350] libplacebo 已移除 ewa_lanczossoft 内核（新版 mpv 报
        // "Failed mapping filter function, no libplacebo analog"），chroma 缩放
        // 换用同为 EWA 系的 ewa_lanczossharp，观感一致且两边都有实现。
        'cscale=ewa_lanczossharp',
        'deband=yes',
        'deband-iterations=2',
        'hdr-compute-peak=yes',
    ],
};

const RENDER_BLOCK_BEGIN = '# ── Fntv-Plus 渲染预设 begin ──';
const RENDER_BLOCK_END = '# ── Fntv-Plus 渲染预设 end ──';

// [lc-1355] 音频响度档 → af 链（写入 mpv-user.conf 托管块）
// loudnorm 输出恒为 192kHz（ffmpeg 滤镜特性），后接 aformat 拉回 48k 避免部分设备协商异常；
// 与「音频源码直通」互斥：直通时 mpv 绕过 af，该档自动失效（面板提示文案已说明）。
const MPV_LOUDNESS_AF: Record<string, string[]> = {
    off: [],
    gain3: ['af=volume=3dB'],
    gain6: ['af=volume=6dB'],
    loudnorm: ['af=loudnorm=I=-16:TP=-1.5:LRA=11,aformat=sample_rates=48000'],
};
const LOUDNESS_BLOCK_BEGIN = '# ── Fntv-Plus 音频响度 begin ──';
const LOUDNESS_BLOCK_END = '# ── Fntv-Plus 音频响度 end ──';

/**
 * 将「默认 MPV 着色器 + ICC 校色」写入 portable_config/mpv-user.conf。
 * 该文件被 mpv.conf 通过 `include=~~/mpv-user.conf` 加载，作为 MPV 启动默认。
 * @param shaderKey 预设 key（'off' 表示不启用任何着色器）
 * @param iccEnabled 是否开启 ICC 自动校色
 */
function writeMpvUserConfig(shaderKey: string, iccEnabled: boolean): void {
    try {
        const shaders = MPV_SHADER_PRESETS[shaderKey] || [];
        const lines: string[] = [
            '# 本文件由「应用设置面板」自动生成（默认着色器 / ICC 校色）。',
            '# 修改后会被重写，请勿手动编辑。',
            '',
            '# N 卡 Smooth Motion（RTX50 驱动级）开关：由 mpv-interp-nvidia.conf 控制',
            '# （应用「插帧引擎=N 卡」时写入 gpu-context=winvk；关闭即清空，不影响其它引擎/默认呈现）',
            'include=~~/mpv-interp-nvidia.conf'
        ];
        for (const s of shaders) {
            lines.push('glsl-shaders-append=~~/shaders/' + s);
        }
        lines.push('icc-profile-auto=' + (iccEnabled ? 'yes' : 'no'));

        // [lc-1069] 追加渲染预设托管块（先剥旧块避免堆叠；预设随 fnConfig 持久化，
        //   shader/ICC 任意重写都会带着当前预设一起写 → 面板各开关互不覆盖）
        const presetKey = fnConfig.getMpvRenderPreset();
        const presetLines = MPV_RENDER_PRESETS[presetKey] || MPV_RENDER_PRESETS.balanced;
        const bIdx = lines.indexOf(RENDER_BLOCK_BEGIN);
        const eIdx = lines.indexOf(RENDER_BLOCK_END);
        if (bIdx >= 0 && eIdx > bIdx) lines.splice(bIdx, eIdx - bIdx + 1);
        lines.push(RENDER_BLOCK_BEGIN, '# preset=' + presetKey);
        for (const pl of presetLines) lines.push(pl);
        lines.push(RENDER_BLOCK_END);

        // [lc-1355] 追加音频响度托管块（同渲染预设策略：剥旧防堆叠，任意重写都带着当前档）
        const loudKey = fnConfig.getMpvAudioLoudness();
        const loudLines = MPV_LOUDNESS_AF[loudKey] || MPV_LOUDNESS_AF.off;
        const lbIdx = lines.indexOf(LOUDNESS_BLOCK_BEGIN);
        const leIdx = lines.indexOf(LOUDNESS_BLOCK_END);
        if (lbIdx >= 0 && leIdx > lbIdx) lines.splice(lbIdx, leIdx - lbIdx + 1);
        lines.push(LOUDNESS_BLOCK_BEGIN, '# mode=' + loudKey);
        for (const ll of loudLines) lines.push(ll);
        lines.push(LOUDNESS_BLOCK_END);
        const content = lines.join('\n') + '\n';

        // ⚠️ 关键修复：同时写入两个目录，确保无论 MPV 处于哪种模式都能生效：
        //   - 便携模式（mpv.exe 同级 portable_config，Windows/macOS 打包态）：读 portable_config/mpv-user.conf
        //   - 标准模式（读系统用户配置目录 AppData/Roaming/mpv 等）：读该目录下的 mpv-user.conf
        // 旧实现只写 portable_config，而 checkAndCopyMpvConfig 仅在首次运行拷贝一次，
        // 之后面板改的着色器写进死目录、正在运行的 MPV 读的是首次拷贝的旧文件 → 「选了不生效」。
        const dirs = [getPortableConfigDir(), getMpvConfigDir()];
        for (const dir of dirs) {
            try {
                if (!fs.existsSync(dir)) {
                    fs.mkdirSync(dir, { recursive: true });
                }
                const target = path.join(dir, 'mpv-user.conf');
                fs.writeFileSync(target, content, 'utf-8');
                logger.info(`MPV 默认配置已写入: ${target} (shader=${shaderKey || 'off'}, icc=${iccEnabled})`);
            } catch (e) {
                logger.error(`写入 mpv-user.conf 失败: ${dir}`, e);
            }
        }
    } catch (error) {
        logger.error('写入 mpv-user.conf 失败:', error);
    }
}

// [lc-486] 写入 MPV 插帧（AI 补帧）配置到 script-opts/fntv_interp.conf。
// 供 fntv_interp.lua 读取（engine / default_on / engine_path）。
// ⚠️ 双写：同时写入 getPortableConfigDir() 与 getMpvConfigDir()（与 writeMpvUserConfig 策略一致），
//   确保便携模式(读 portable_config)与标准模式(读 AppData/Roaming/mpv)下都能读到。
export function writeInterpConfig(enabled: boolean, engine: string, enginePath: string): void {
    try {
        const eng = (engine === 'svp' || engine === 'rife' || engine === 'builtin' || engine === 'nvidia') ? engine : 'auto';
        const lines = [
            '# Fntv-Plus 插帧（AI 补帧）配置',
            '# 由应用「设置面板 > 插帧（AI 补帧）」写入，请勿手动编辑（修改会被覆盖）。',
            '',
            '# 插帧引擎：auto(自动探测 SVP→RIFE→内置) / svp / rife / builtin / nvidia(RTX50 驱动级 Smooth Motion)',
            'engine=' + eng,
            '',
            '# 启动播放时即开启插帧（默认关=否）',
            'default_on=' + (enabled ? 'yes' : 'no'),
            '',
            '# 引擎路径：SVP 安装目录 或 rife-ncnn-vulkan 可执行文件完整路径',
            '# 留空=自动探测（SVP 走默认管道；RIFE 需在 mpv.conf 的 [fntv-interp-rife] 中配好 vf）',
            'engine_path=' + (enginePath || ''),
            ''
        ];
        const content = lines.join('\n');
        const dirs = [getPortableConfigDir(), getMpvConfigDir()];
        for (const dir of dirs) {
            try {
                const scriptOptsDir = path.join(dir, 'script-opts');
                if (!fs.existsSync(scriptOptsDir)) fs.mkdirSync(scriptOptsDir, { recursive: true });
                const target = path.join(scriptOptsDir, 'fntv_interp.conf');
                fs.writeFileSync(target, content, 'utf-8');
                logger.info(`[插帧] 配置已写入: ${target} (engine=${eng}, default_on=${enabled})`);
            } catch (e) {
                logger.error(`[插帧] 写入 fntv_interp.conf 失败: ${dir}`, e);
            }
        }
        // [lc-490] N 卡 Smooth Motion 需 mpv 走 Vulkan 呈现（gpu-context=winvk）才能被 NVIDIA 驱动接管；
        // d3d11 上下文 NVIDIA SM 不生效。gpu-context 是启动项，无法运行时切换，故写入独立被 include 的
        // mpv-interp-nvidia.conf（由 mpv-user.conf 的 include 加载），下次启动生效。仅引擎=nvidia 时写入。
        writeInterpNvidiaContext(engine === 'nvidia');
    } catch (error) {
        logger.error('[插帧] 写入 fntv_interp.conf 失败:', error);
    }
}

// [lc-490] 写入/清空 N 卡 Smooth Motion 所需的 Vulkan 呈现上下文配置。
// 仅当引擎= nvidia 时写入 gpu-context=winvk + video-sync=audio（NVIDIA 驱动级 SM 的实测可用组合）；
// 关闭时写入空文件（include 存在但无内容，不影响默认 D3D11 呈现与其它引擎）。
function writeInterpNvidiaContext(enabled: boolean): void {
    try {
        const content = enabled
            ? '# Fntv-Plus · N 卡 Smooth Motion（RTX50 驱动级）\n'
              + '# 仅当「插帧引擎=N 卡」时由应用写入；请勿手动编辑。\n'
              + '# 启用 Vulkan 呈现上下文，NVIDIA 驱动级 Smooth Motion 才能对视频层做帧生成（d3d11 不生效）。\n'
              + 'gpu-context=winvk\n'
              + 'video-sync=audio\n'
            : '# Fntv-Plus · N 卡 Smooth Motion 未启用（空）\n';
        const dirs = [getPortableConfigDir(), getMpvConfigDir()];
        for (const dir of dirs) {
            try {
                if (!fs.existsSync(dir)) fs.mkdirSync(dir, { recursive: true });
                const target = path.join(dir, 'mpv-interp-nvidia.conf');
                fs.writeFileSync(target, content, 'utf-8');
                logger.info(`[插帧] N 卡上下文配置已写入: ${target} (enabled=${enabled})`);
            } catch (e) {
                logger.error(`[插帧] 写入 mpv-interp-nvidia.conf 失败: ${dir}`, e);
            }
        }
    } catch (error) {
        logger.error('[插帧] 写入 mpv-interp-nvidia.conf 失败:', error);
    }
}

// 写入 MPV B站弹幕搜索开关到 script-opts/uosc_danmaku.conf
// 同时控制 bili_search_enabled(手动搜索门控) 与 auto_load_extra(自动补源/B站自动搜索)，
// 两者同开同关，确保关闭开关后既不能手动搜、也不会自动加载 B站弹幕。
// ⚠️ 双写：同时写入 getPortableConfigDir() 与 getMpvConfigDir()（用户配置目录 AppData/Roaming/mpv）。
//   mpv 在「便携模式」(CWD=exe目录) 读 portable_config，在「标准模式」读用户配置目录；
//   只写一处会导致另一模式下 auto_load_extra 不生效（典型表现：换台机器/某些环境 B站 永不自动搜索）。
//   这与 writeMpvUserConfig / writeBiliDanmakuStyle 的双写策略一致（lc-094 教训：着色器单写导致不生效）。
// 保留 conf 中其他选项，仅替换/追加这两个键。
function writeBiliSearchEnabled(enabled: boolean): void {
    try {
        const val = enabled ? 'yes' : 'no';
        const dirs = [getPortableConfigDir(), getMpvConfigDir()];
        for (const dir of dirs) {
            try {
                const scriptOptsDir = path.join(dir, 'script-opts');
                if (!fs.existsSync(scriptOptsDir)) {
                    fs.mkdirSync(scriptOptsDir, { recursive: true });
                }
                const target = path.join(scriptOptsDir, 'uosc_danmaku.conf');
                let lines: string[] = [];
                if (fs.existsSync(target)) {
                    lines = fs.readFileSync(target, 'utf-8').split(/\r?\n/);
                }
                // 移除已存在的 bili_search_enabled / auto_load_extra 行，以及旧的开关注释行（防止注释无限堆叠）
                lines = lines.filter(l => !/^\s*(bili_search_enabled|auto_load_extra)\s*=/.test(l)
                    && !/^#\s*B站弹幕搜索开关/.test(l));
                // 去掉末尾多余空行
                while (lines.length > 0 && lines[lines.length - 1].trim() === '') lines.pop();
                lines.push('# B站弹幕搜索开关（由应用设置面板控制，同时控制自动补源 auto_load_extra）');
                lines.push('bili_search_enabled=' + val);
                lines.push('auto_load_extra=' + val);
                fs.writeFileSync(target, lines.join('\n') + '\n', 'utf-8');
                logger.info(`MPV B站弹幕搜索开关已写入: ${target} (enabled=${enabled}, auto_load_extra=${val})`);
            } catch (e) {
                logger.error(`写入 uosc_danmaku.conf (B站弹幕搜索开关) 失败: ${dir}`, e);
            }
        }
    } catch (error) {
        logger.error('写入 B站弹幕搜索开关失败:', error);
    }
}

// [lc-1068] 写入 thumbfast.conf：mpv_path 指向实际 mpv 二进制（uosc 进度条悬停缩略图
//   依赖 thumbfast 子进程用同款 mpv 生成预览帧；双写 portable + 用户配置目录，与 smart_skip 策略一致）
function writeThumbfastConf(playerPath: string): void {
    try {
        const conf = 'mpv_path=' + playerPath;
        const dirs = [getPortableConfigDir(), getMpvConfigDir()];
        for (const dir of dirs) {
            try {
                const scriptOptsDir = path.join(dir, 'script-opts');
                if (!fs.existsSync(scriptOptsDir)) fs.mkdirSync(scriptOptsDir, { recursive: true });
                fs.writeFileSync(path.join(scriptOptsDir, 'thumbfast.conf'), conf, 'utf8');
                logger.log('thumbfast.conf 已写入: ' + path.join(scriptOptsDir, 'thumbfast.conf') + ' (mpv_path=' + playerPath + ')');
            } catch (e: any) {
                logger.error('写入 thumbfast.conf 失败 (' + dir + '):', e && e.message);
            }
        }
    } catch (error) {
        logger.error('写入 thumbfast.conf 失败:', error);
    }
}

// 写入智能跳过片头片尾开关到 script-opts/smart_skip.conf（由应用「插件」面板控制）
// ⚠️ 同样双写 portable_config 与用户配置目录（AppData/Roaming/mpv），
//   确保 mpv 在便携/标准两种模式下都读到正确的 enabled（lc-094 教训）。
function writeSmartSkipEnabled(enabled: boolean): void {
    try {
        const val = enabled ? 'yes' : 'no';
        const dirs = [getPortableConfigDir(), getMpvConfigDir()];
        for (const dir of dirs) {
            try {
                const scriptOptsDir = path.join(dir, 'script-opts');
                if (!fs.existsSync(scriptOptsDir)) {
                    fs.mkdirSync(scriptOptsDir, { recursive: true });
                }
                const target = path.join(scriptOptsDir, 'smart_skip.conf');
                let lines: string[] = [];
                if (fs.existsSync(target)) {
                    lines = fs.readFileSync(target, 'utf-8').split(/\r?\n/);
                }
                // 移除已存在的 enabled 行及相关注释，避免重复堆叠
                lines = lines.filter(l => !/^\s*enabled\s*=/.test(l)
                    && !/^#\s*智能跳过片头片尾开关/.test(l)
                    && !/^#\s*开启后：/.test(l));
                while (lines.length > 0 && lines[lines.length - 1].trim() === '') lines.pop();
                lines.push('# 智能跳过片头片尾开关（由应用「插件」面板控制）');
                lines.push('enabled=' + val);
                fs.writeFileSync(target, lines.join('\n') + '\n', 'utf-8');
                logger.info(`智能跳过片头片尾开关已写入: ${target} (enabled=${enabled})`);
            } catch (e) {
                logger.error(`写入 smart_skip.conf (智能跳过开关) 失败: ${dir}`, e);
            }
        }
    } catch (error) {
        logger.error('写入 智能跳过片头片尾开关失败:', error);
    }
}

// 写入 B站弹幕聚合阈值到 script-opts/uosc_danmaku.conf（由应用设置面板控制）
function writeBiliAggregateThreshold(threshold: number): void {
    try {
        const dir = getPortableConfigDir();
        const scriptOptsDir = path.join(dir, 'script-opts');
        if (!fs.existsSync(scriptOptsDir)) {
            fs.mkdirSync(scriptOptsDir, { recursive: true });
        }
        const target = path.join(scriptOptsDir, 'uosc_danmaku.conf');
        let lines: string[] = [];
        if (fs.existsSync(target)) {
            lines = fs.readFileSync(target, 'utf-8').split(/\r?\n/);
        }
        // 移除已存在的 aggregate_threshold 行及旧注释，避免重复堆叠
        lines = lines.filter(l => !/^\s*aggregate_threshold\s*=/.test(l)
            && !/^#\s*B站弹幕聚合阈值/.test(l));
        while (lines.length > 0 && lines[lines.length - 1].trim() === '') lines.pop();
        const t = Number(threshold) || 0;
        lines.push('# B站弹幕聚合阈值（单视频弹幕<此值时合并同类候选；0=禁用）');
        lines.push('aggregate_threshold=' + t);
        fs.writeFileSync(target, lines.join('\n') + '\n', 'utf-8');
        logger.info(`MPV B站弹幕聚合阈值已写入: ${target} (threshold=${t})`);
    } catch (error) {
        logger.error('写入 uosc_danmaku.conf (aggregate_threshold) 失败:', error);
    }
}

// [lc-1226] 把明文凭证加密成 dandanplay.lua 能解开的 AES-256-ECB 密文（base64）。
// 契约：密钥 = main.lua 的 KEY（table_to_zero_indexed({0x00..0x1f})，实际是 32 个 0x1f）；
//       零填充到 16 字节整数倍、不启用 PKCS7（与 Lua 侧 chunks() 的行为一一对应）。
// 为什么写密文而不是明文：uosc_danmaku.conf 既被 git 跟踪、又随安装包分发，明文等于把
// 用户的专属密钥公开传播出去——这正是内置凭证要密文内嵌的同一个理由。
function ddEncryptForLua(plain: string): string {
    const KEY = Buffer.alloc(32, 0x1f);
    let buf = Buffer.from(String(plain || ''), 'utf8');
    const rem = buf.length % 16;
    if (rem !== 0) buf = Buffer.concat([buf, Buffer.alloc(16 - rem)]);
    const cipher = crypto.createCipheriv('aes-256-ecb', KEY, null);
    cipher.setAutoPadding(false);
    return Buffer.concat([cipher.update(buf), cipher.final()]).toString('base64');
}

// [lc-1226] 写入弹弹play 开放 API 自定义凭证到 script-opts/uosc_danmaku.conf（由应用设置面板控制）。
// 背景：脚本内置了一套兜底凭证（AppId + Secret，AES 密文内嵌于 apis/dandanplay.lua，
// 固定用 1 号密钥、不做自动轮换以免被视为绕过官方处置）。
// [lc-1228] 内置凭证是【兜底】：未配自定义凭证时，弹幕库（/comment）先让自建源与内置 B站 去取，
// 两者都拿不到才回退内置凭证——按官方「按需使用」约定减少共享配额消耗。
// 这里写的是【用户自定义凭证】：填了则弹弹play 立即参与并优先取弹幕库（消耗用户自己的配额）。
// ⚠️ 双写 portable_config 与用户配置目录（AppData/Roaming/mpv），同 writeBiliSearchEnabled 的 lc-094 教训。
// 两项任一为空 → 从 conf 删除对应键（dandanplay.lua 端回到兜底状态）。
// mpv 在每次启动(每次播放拉起的新进程)读取 script-opts → 下次播放生效，无需重启应用。
function writeDandanplayCredentials(appId: string, appSecret: string): void {
    try {
        const id = String(appId || '').trim();
        const secret = String(appSecret || '').trim();
        const dirs = [getPortableConfigDir(), getMpvConfigDir()];
        for (const dir of dirs) {
            try {
                const scriptOptsDir = path.join(dir, 'script-opts');
                if (!fs.existsSync(scriptOptsDir)) {
                    fs.mkdirSync(scriptOptsDir, { recursive: true });
                }
                const target = path.join(scriptOptsDir, 'uosc_danmaku.conf');
                let lines: string[] = [];
                if (fs.existsSync(target)) {
                    lines = fs.readFileSync(target, 'utf-8').split(/\r?\n/);
                }
                // 移除已存在的凭证键及旧注释，避免重复堆叠。
                // 明文键（dandanplay_app_id/secret，历史版本写法）一并清掉：升级后首次同步即自动清除
                // 用户机器上残留的明文密钥（同 lc-1104 对 danmu_api 地址的处理）。
                lines = lines.filter(l => !/^\s*dandanplay_app_(id|secret)(_enc)?\s*=/.test(l)
                    && !/^#.*弹弹play.*凭证/.test(l));
                while (lines.length > 0 && lines[lines.length - 1].trim() === '') lines.pop();
                if (id && secret) {
                    lines.push('# 弹弹play 开放 API 专属凭证（由应用设置面板控制，AES 加密存储；留空/清除即回落脚本内置凭证）');
                    lines.push('dandanplay_app_id_enc=' + ddEncryptForLua(id));
                    lines.push('dandanplay_app_secret_enc=' + ddEncryptForLua(secret));
                }
                fs.writeFileSync(target, lines.join('\n') + '\n', 'utf-8');
                logger.info(`MPV 弹弹play凭证已写入: ${target} (appId=${id ? id.slice(0, 2) + '***' : '(空,回落内置)'})`);
            } catch (e) {
                logger.error(`写入 uosc_danmaku.conf (弹弹play凭证) 失败: ${dir}`, e);
            }
        }
    } catch (error) {
        logger.error('写入弹弹play凭证失败:', error);
    }
}

// [lc-1101] 写入「自建弹幕接口（danmu_api）」开关到 script-opts/uosc_danmaku.conf。
// 这个键只是 Lua 侧的【闸门】：danmu_api_enabled 让自动补源在「B站弹幕搜索」关闭时也能触发
// （main.lua:891/954、apis/dandanplay.lua:53 的 `auto_load_extra or danmu_api_enabled`），
// 真正的服务地址由主进程从 config.json 读（danmuApi.ts），Lua 不参与 HTTP。
// ⚠️ 双写 portable_config 与用户配置目录（AppData/Roaming/mpv），同 writeDandanplayCredentials 的 lc-094 教训。
// 未启用或地址为空 → 从 conf 删键（options.lua 默认 danmu_api_enabled=false，即完全走内置 B站 链路）。
// ⚠️ [lc-1104] 绝不把 base 写进 conf：它 Lua 侧零读取，而 portable_config 是 git 跟踪且随安装包
//   分发的目录 —— 实测已把用户内网地址连同 TOKEN 路径写进仓库工作区和 win-unpacked。地址只存 config.json。
//   下面的 filter 仍会清掉历史版本残留的 danmu_api_base 行，升级后用户机器上的明文 TOKEN 自动消失。
function writeDanmuApiConf(enabled: boolean, base: string): void {
    try {
        const on = !!enabled;
        const hasBase = !!String(base || '').trim();
        const dirs = [getPortableConfigDir(), getMpvConfigDir()];
        for (const dir of dirs) {
            try {
                const scriptOptsDir = path.join(dir, 'script-opts');
                if (!fs.existsSync(scriptOptsDir)) {
                    fs.mkdirSync(scriptOptsDir, { recursive: true });
                }
                const target = path.join(scriptOptsDir, 'uosc_danmaku.conf');
                let lines: string[] = [];
                if (fs.existsSync(target)) {
                    lines = fs.readFileSync(target, 'utf-8').split(/\r?\n/);
                }
                lines = lines.filter(l => !/^\s*(danmu_api_enabled|danmu_api_base)\s*=/.test(l)
                    && !/^#\s*自建弹幕接口/.test(l));
                while (lines.length > 0 && lines[lines.length - 1].trim() === '') lines.pop();
                if (on && hasBase) {
                    lines.push('# 自建弹幕接口（danmu_api）：由应用设置面板控制，作为弹幕优选源；未命中自动降级内置B站弹幕');
                    lines.push('danmu_api_enabled=yes');
                }
                fs.writeFileSync(target, lines.join('\n') + '\n', 'utf-8');
                logger.info(`MPV 自建弹幕接口已写入: ${target} (enabled=${on}, 地址已配置=${hasBase})`);
            } catch (e) {
                logger.error(`写入 uosc_danmaku.conf (自建弹幕接口) 失败: ${dir}`, e);
            }
        }
    } catch (error) {
        logger.error('写入自建弹幕接口配置失败:', error);
    }
}

// 写入 B站弹幕「样式与过滤」到 script-opts/uosc_danmaku.conf（覆盖 fontsize/opacity/outline/shadow/bold/displayarea/max_screen_danmaku/blacklist_path）
// 同时把屏蔽词写入 <portable_config>/danmaku_blacklist.txt（用 ~~ 相对路径引用，MPV 自动解析到当前配置目录），
// 把弹幕屏蔽类型写入 <portable_config>/scripts/uosc_danmaku/danmaku_block_types.json（bili_danmaku.js 启动时读取并过滤）。
// 保留 conf 中其他选项（bili_search_enabled / auto_load_extra / aggregate_threshold 等由各自函数管理）。
function writeBiliDanmakuStyle(): void {
    try {
        const opacity = fnConfig.getBiliDanmakuOpacity();
        const fontsize = fnConfig.getBiliDanmakuFontSize();
        const outline = fnConfig.getBiliDanmakuOutline();
        const shadow = fnConfig.getBiliDanmakuShadow();
        const bold = fnConfig.getBiliDanmakuBold();
        const displayarea = fnConfig.getBiliDanmakuDisplayArea();
        const maxScreen = fnConfig.getBiliDanmakuMaxScreen();
        const blacklist = fnConfig.getBiliDanmakuBlacklist() || '';

        const dirs = [getPortableConfigDir(), getMpvConfigDir()];
        for (const dir of dirs) {
            try {
                const scriptOptsDir = path.join(dir, 'script-opts');
                if (!fs.existsSync(scriptOptsDir)) fs.mkdirSync(scriptOptsDir, { recursive: true });
                const target = path.join(scriptOptsDir, 'uosc_danmaku.conf');
                let lines: string[] = [];
                if (fs.existsSync(target)) {
                    lines = fs.readFileSync(target, 'utf-8').split(/\r?\n/);
                }
                // 移除已存在的相关键及旧注释，避免堆叠
                lines = lines.filter(l => !/^\s*(fontsize|opacity|outline|shadow|bold|displayarea|max_screen_danmaku|blacklist_path)\s*=/.test(l)
                    && !/^#\s*B站弹幕(样式|透明度|字号|描边|阴影|粗体|显示区域|同屏上限|屏蔽词)/.test(l));
                while (lines.length > 0 && lines[lines.length - 1].trim() === '') lines.pop();

                const blacklistFile = path.join(dir, 'danmaku_blacklist.txt');
                // 屏蔽词：按行写入（支持 lua 正则）；空则清空文件引用
                const words = blacklist.split(/\r?\n/).map(w => w.trim()).filter(Boolean);
                fs.writeFileSync(blacklistFile, words.join('\n') + (words.length ? '\n' : ''), 'utf-8');

                const push = (label: string, key: string, val: string) => {
                    lines.push(`# B站弹幕${label}`);
                    lines.push(`${key}=${val}`);
                };
                push('透明度', 'opacity', String(opacity));
                push('字号', 'fontsize', String(fontsize));
                push('描边', 'outline', String(outline));
                push('阴影', 'shadow', String(shadow));
                push('粗体', 'bold', bold ? 'yes' : 'no');
                push('显示区域', 'displayarea', String(displayarea));
                push('同屏上限', 'max_screen_danmaku', String(maxScreen));
                push('屏蔽词', 'blacklist_path', '~~/danmaku_blacklist.txt');
                fs.writeFileSync(target, lines.join('\n') + '\n', 'utf-8');
                logger.info(`B站弹幕样式已写入: ${target} (opacity=${opacity},size=${fontsize},outline=${outline},shadow=${shadow},bold=${bold},area=${displayarea},max=${maxScreen},blacklist=${words.length}词)`);
                // 弹幕屏蔽类型：写入 scripts/uosc_danmaku/danmaku_block_types.json（bili_danmaku.js 启动时读取并过滤）
                const blockTypes = fnConfig.getBiliDanmakuBlockTypes();
                const uoscDir = path.join(dir, 'scripts', 'uosc_danmaku');
                if (!fs.existsSync(uoscDir)) fs.mkdirSync(uoscDir, { recursive: true });
                fs.writeFileSync(path.join(uoscDir, 'danmaku_block_types.json'), JSON.stringify(blockTypes), 'utf-8');
            } catch (e) {
                logger.error(`写入 uosc_danmaku.conf 失败: ${dir}`, e);
            }
        }
    } catch (error) {
        logger.error('写入 B站弹幕样式失败:', error);
    }
}

/** [lc-1252] 仅写屏蔽相关（blockTypes json + 屏蔽词文件 + conf 的 blacklist_path 键）。
 *  样式键（fontsize/opacity/outline/shadow/bold/displayarea/scrolltime）自 lc-1252 起由
 *  MPV 弹幕样式菜单直接持久化到 conf——屏蔽词保存若整写样式键，会把用户在播放器里
 *  调好的值冲回 fnConfig 冻结值（lc-215 起设置面板已无样式行，fnConfig 样式字段无人更新）。 */
function writeBiliDanmakuBlockOnly(): void {
    try {
        const blacklist = fnConfig.getBiliDanmakuBlacklist() || '';
        const blockTypes = fnConfig.getBiliDanmakuBlockTypes();
        const dirs = [getPortableConfigDir(), getMpvConfigDir()];
        for (const dir of dirs) {
            try {
                const scriptOptsDir = path.join(dir, 'script-opts');
                if (!fs.existsSync(scriptOptsDir)) fs.mkdirSync(scriptOptsDir, { recursive: true });
                const target = path.join(scriptOptsDir, 'uosc_danmaku.conf');
                // 屏蔽词：按行写入（支持 lua 正则）；空则清空文件引用
                const blacklistFile = path.join(dir, 'danmaku_blacklist.txt');
                const words = blacklist.split(/\r?\n/).map(w => w.trim()).filter(Boolean);
                fs.writeFileSync(blacklistFile, words.join('\n') + (words.length ? '\n' : ''), 'utf-8');
                // conf：仅确保 blacklist_path 键存在，不触碰任何样式键
                let lines: string[] = [];
                if (fs.existsSync(target)) {
                    lines = fs.readFileSync(target, 'utf-8').split(/\r?\n/);
                }
                const hasBlacklistPath = lines.some(l => /^\s*blacklist_path\s*=/.test(l));
                if (!hasBlacklistPath) {
                    while (lines.length > 0 && lines[lines.length - 1].trim() === '') lines.pop();
                    lines.push('# B站弹幕屏蔽词');
                    lines.push('blacklist_path=~~/danmaku_blacklist.txt');
                    fs.writeFileSync(target, lines.join('\n') + '\n', 'utf-8');
                }
                // 弹幕屏蔽类型：写入 scripts/uosc_danmaku/danmaku_block_types.json（bili_danmaku.js 启动时读取并过滤）
                const uoscDir = path.join(dir, 'scripts', 'uosc_danmaku');
                if (!fs.existsSync(uoscDir)) fs.mkdirSync(uoscDir, { recursive: true });
                fs.writeFileSync(path.join(uoscDir, 'danmaku_block_types.json'), JSON.stringify(blockTypes), 'utf-8');
            } catch (e) {
                logger.error(`写入弹幕屏蔽配置失败: ${dir}`, e);
            }
        }
        logger.info('B站弹幕屏蔽配置已更新（样式键未触碰）');
    } catch (error) {
        logger.error('写入 B站弹幕屏蔽配置失败:', error);
    }
}

// 插件初始化函数
function init(): void {
    logger.info('Initializing MPV Config Plugin...');
    // 只在macOS上执行
    if (process.platform === 'win32' || process.platform === 'linux') {
        return;
    }

    startConfigCheck();
    // 应用退出前停止检查
    app.on('before-quit', () => {
        stopConfigCheck();
    });
}


/** [lc-1069] 应用渲染预设：持久化 + 重写 mpv-user.conf(带着当前着色器/ICC 设置)。
 *  vo 变更需 MPV 进程重启生效 → 面板提示「重启应用后生效」。 */
export function applyRenderPreset(preset: string): void {
    const p = preset === 'perf' || preset === 'quality' ? preset : 'balanced';
    fnConfig.setMpvRenderPreset(p);
    writeMpvUserConfig(fnConfig.getMpvDefaultShader(), fnConfig.getMpvIccEnabled() !== false);
}

/** [lc-1355] 应用音频响度档：持久化 + 重写 mpv-user.conf（af 链随下次播放生效）。 */
export function applyAudioLoudness(mode: string): void {
    const m = mode === 'gain3' || mode === 'gain6' || mode === 'loudnorm' ? mode : 'off';
    fnConfig.setMpvAudioLoudness(m);
    writeMpvUserConfig(fnConfig.getMpvDefaultShader(), fnConfig.getMpvIccEnabled() !== false);
}

/** [lc-1069] 确保 input.conf 含视频统计面板绑定(F 键, mpv 内置 stats 脚本)。
 *  存量安装的 input.conf 是首启拷贝的旧版 → 幂等补一行(带 uosc 菜单注释)。 */
export function ensureStatsKeyBinding(): void {
    const line = 'F          script-binding stats/display-stats-toggle                                      #menu: 播放 > 视频统计信息';
    const marker = 'script-binding stats/display-stats-toggle';
    const dirs = [getPortableConfigDir(), getMpvConfigDir()];
    for (const dir of dirs) {
        try {
            const target = path.join(dir, 'input.conf');
            const existing = fs.existsSync(target) ? fs.readFileSync(target, 'utf-8') : '';
            if (existing.indexOf(marker) >= 0) continue;
            fs.writeFileSync(target, existing.replace(/\s*$/, '') + '\n\n' + line + '\n', 'utf-8');
            logger.info('已补写视频统计面板快捷键(F): ' + target);
        } catch (e: any) {
            logger.warn('补写 stats 快捷键失败 (' + dir + '):', e && e.message);
        }
    }
}

export {
    init,
    getPortableConfigDir,
    writeMpvUserConfig,
    writeBiliSearchEnabled,
    writeSmartSkipEnabled,
    writeThumbfastConf,
    writeBiliAggregateThreshold,
    writeBiliDanmakuStyle,
    writeBiliDanmakuBlockOnly,
    writeDandanplayCredentials,
    writeDanmuApiConf
};