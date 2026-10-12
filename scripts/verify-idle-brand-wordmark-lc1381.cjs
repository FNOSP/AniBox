/**
 * [lc-1381] 空闲品牌水印文字标静态验证（零 CPU，不启 mpv / 浏览器）。
 *
 * 被测对象：third_party/fntv-mpv/portable_config/scripts/uosc/lib/utils.lua 的 render() 里
 * 拼出的 ASS 文字标（uosc 空闲启动时画在屏幕中央的「mpv-Yaozhi」→ 现应为「Fntv-Plus」）。
 *
 * 做法：把 brand_name 表达式按 Lua 单引号字符串 + `..` 拼接语义还原（含 \\ 反转义），
 * 代入 main.lua 的品牌色（config.color.idle_brand_glow / idle_brand_text），断言最终 ASS 串。
 *
 * 双向验证（防空转）：
 *   git show HEAD:third_party/fntv-mpv/portable_config/scripts/uosc/lib/utils.lua > /tmp/old_utils.lua
 *   node scripts/verify-idle-brand-wordmark-lc1381.cjs /tmp/old_utils.lua   → 必须 FAIL（可见文字为 mpv-Yaozhi）
 *   node scripts/verify-idle-brand-wordmark-lc1381.cjs                      → 当前工作区必须 PASS
 */
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const root = path.join(__dirname, '..');
const utilsPath = process.argv[2] || path.join(root, 'third_party/fntv-mpv/portable_config/scripts/uosc/lib/utils.lua');
const mainPath = path.join(root, 'third_party/fntv-mpv/portable_config/scripts/uosc/main.lua');

const utilsSrc = fs.readFileSync(utilsPath, 'utf8');
const mainSrc = fs.readFileSync(mainPath, 'utf8');

const failures = [];
const check = (ok, label) => {
    console.log(`${ok ? 'PASS' : 'FAIL'}  ${label}`);
    if (!ok) failures.push(label);
};

// ── 1. 取品牌色（main.lua：serialize_rgba('rrggbb').color = ASS 的 AABBGGRR，BGR 小端序） ──
const colorOf = (key) => {
    const m = mainSrc.match(new RegExp(`${key}\\s*=\\s*serialize_rgba\\('([0-9a-fA-F]{6})'\\)\\.color`));
    if (!m) return null;
    const [r, g, b] = [m[1].slice(0, 2), m[1].slice(2, 4), m[1].slice(4, 6)];
    return `${b}${g}${r}`.toUpperCase();
};
const glow = colorOf('idle_brand_glow');
const text = colorOf('idle_brand_text');
check(!!glow && !!text, `取到品牌色 idle_brand_glow=H${glow} idle_brand_text=H${text}`);

// ── 2. 取出 brand_name 表达式（Lua `..` 续行，到后面的 ass:txt( 调用为止） ──
const brandBlock = utilsSrc.match(/local brand_name = ([\s\S]*?)ass:txt\(/);
check(!!brandBlock, '找到 local brand_name = … 表达式');

// ── 3. 按 Lua 语义求值：单引号串（\\ → \、\' → '、其余原样）+ 两个已知色值 ──
function luaUnescapeSingleQuoted(raw) {
    let out = '';
    for (let i = 0; i < raw.length; i++) {
        const c = raw[i];
        if (c === '\\' && i + 1 < raw.length) {
            const n = raw[++i];
            out += n === '\\' ? '\\' : n === "'" ? "'" : n; // Lua 5.1：未识别转义保留原字符
        } else {
            out += c;
        }
    }
    return out;
}

function evalLuaConcat(expr) {
    return expr
        .replace(/\s+/g, ' ')
        .split('..')
        .map((p) => p.trim())
        .filter((p) => p.length > 0)
        .map((p) => {
            const lit = p.match(/^'((?:[^'\\]|\\.)*)'$/);
            if (lit) return luaUnescapeSingleQuoted(lit[1]);
            if (p === 'config.color.idle_brand_glow') return glow;
            if (p === 'config.color.idle_brand_text') return text;
            throw new Error(`表达式含未识别片段: ${p}`);
        })
        .join('');
}

let rendered = null;
try {
    rendered = evalLuaConcat(brandBlock ? brandBlock[1] : '');
} catch (e) {
    check(false, `表达式求值: ${e.message}`);
}

// ── 4. 断言最终 ASS 串（含 \\1c 覆盖标签，即上屏真实形态） ──
const expected = `{\\1c&H${glow}&}Fntv-{\\1c&H${text}&}Plus`;
check(rendered === expected, `ASS 文字标 == ${expected}（实际 ${JSON.stringify(rendered)}）`);

const visible = rendered ? rendered.replace(/\{[^}]*\}/g, '') : '';
check(visible === 'Fntv-Plus', `上屏可见文字 == "Fntv-Plus"（实际 "${visible}"）`);
check(!/Yaozhi/i.test(rendered || ''), '文字标不再残留 Yaozhi 字样');
check((rendered || '').split('{\\1c&H').length - 1 === 2, '保持上游双色版式（两段各带 1c 颜色覆盖）');

// ── 5. 残留扫描：mpv 配置内所有 .lua/.conf 的「上屏/日志外」旧品牌字样应为零 ──
//    排除：注释行、出处/许可说明（md 文档与 THIRD-PARTY-LICENSES 的署名义务）。
const isComment = (code) => /^\s*(--|#|\/\*|\*)/.test(code);
const isAttribution = (s) => /对照|参考|对齐|移植自|来源|沿用上游|许可|License|第三方|原样捆绑/.test(s);
// 已知惰性遗留（显式登记，不隐藏——每条都会在输出里打印）：
//   · yaozhi-release.lua：孤儿元数据模块，仓库内无任何脚本 require，不上屏（记录上游整合包版本）
//   · menus.lua 的 '杳知' 匹配分支：顶层菜单早已改名「Fntv-Plus」，这三处注入（菜单 min_width /
//     Atmos 实验入口 / 支持入口置底）在当前构建下全部不生效（Atmos sidecar 未随包、菜单内无支持项），
//     改字符串会「复活」它们并改动菜单布局 → 属行为决策，留给用户定夺，不在 lc-1381 内动。
//   · menus.lua:2781：Atmos 实验子菜单脚注（同一未随包的 sidecar，不可达）
const KNOWN_INERT = [
    /^third_party\/fntv-mpv\/portable_config\/script-modules\/yaozhi-release\.lua:/,
    /^third_party\/fntv-mpv\/portable_config\/scripts\/uosc\/lib\/menus\.lua:2781:/,
    /^third_party\/fntv-mpv\/portable_config\/scripts\/uosc\/lib\/menus\.lua:313:/,
    /^third_party\/fntv-mpv\/portable_config\/scripts\/uosc\/lib\/menus\.lua:329:/,
    /^third_party\/fntv-mpv\/portable_config\/scripts\/uosc\/lib\/menus\.lua:354:/,
    /^third_party\/fntv-mpv\/portable_config\/scripts\/uosc\/lib\/menus\.lua:362:/,
];

let hits = [];
let inert = [];
try {
    const grepOut = execSync(
        'git grep -n "Yaozhi\\|杳知\\|Yaoplayer" -- "third_party/fntv-mpv/portable_config/*.lua" "third_party/fntv-mpv/portable_config/*.conf"',
        { cwd: root, encoding: 'utf8' }
    );
    for (const line of grepOut.split('\n')) {
        if (!line.trim()) continue;
        const norm = line.replace(/\\/g, '/');
        const code = line.split(':').slice(2).join(':');
        if (KNOWN_INERT.some((re) => re.test(norm))) { inert.push(norm); continue; }
        if (isComment(code) || isAttribution(code)) continue;
        hits.push(norm);
    }
} catch (e) {
    if (e.status !== 1) throw e; // grep 无命中时退出码 1
}
check(hits.length === 0, `上屏旧品牌字样清零（剩余命中 ${hits.length} 条）`);
hits.forEach((l) => console.log('      ↳', l));
console.log(`INFO  已知惰性遗留 ${inert.length} 条（不上屏/不生效，见脚本头注释）`);
inert.forEach((l) => console.log('      ·', l));

// ── 6. 结构性断言：本轮触碰的文件里，非注释代码（归一化品牌词 + 压空白后）必须与 HEAD 逐字相同 ──
//    即「只改了品牌字样与全行注释」，不可能改坏逻辑/语法结构。
const CHANGED_FILES = [
    'third_party/fntv-mpv/portable_config/scripts/uosc/lib/utils.lua',
    'third_party/fntv-mpv/portable_config/scripts/uosc/main.lua',
    'third_party/fntv-mpv/portable_config/scripts/uosc/lib/menus.lua',
    'third_party/fntv-mpv/portable_config/script-modules/ai-background.lua',
    'third_party/fntv-mpv/portable_config/scripts/music-mode.lua',
    'third_party/fntv-mpv/portable_config/scripts/image-subs-brightness.lua',
    'third_party/fntv-mpv/portable_config/script-opts/uosc.conf',
    'third_party/fntv-mpv/portable_config/script-opts/image_subs_brightness.conf',
];
const BRAND_RE = /(mpv-yaozhi|yaoplayer|yaozhi|fntv-plus|fntv-|mpv-|杳知|plus|启动界面|品牌水印)/gi;
const codeShape = (src) => src
    .split('\n')
    .filter((l) => !/^\s*(--|#)/.test(l))   // 允许全行注释（Lua/conf）差异
    .map((l) => l.replace(BRAND_RE, '<BRAND>').replace(/\s+/g, ' ').trim())
    .filter((l) => l.length > 0)
    .join('\n');

for (const file of CHANGED_FILES) {
    const rel = file.replace(/\\/g, '/');
    let oldSrc;
    try {
        oldSrc = execSync(`git show HEAD:${rel}`, { cwd: root, encoding: 'utf8' });
    } catch (e) {
        check(false, `读取 HEAD 版本失败: ${rel}`);
        continue;
    }
    const newSrc = fs.readFileSync(path.join(root, rel), 'utf8');
    const same = codeShape(oldSrc) === codeShape(newSrc);
    check(same, `代码结构未变（仅品牌字样）: ${rel}`);
    if (!same) {
        const a = codeShape(oldSrc).split('\n');
        const b = codeShape(newSrc).split('\n');
        for (let i = 0; i < Math.max(a.length, b.length); i++) {
            if (a[i] !== b[i]) console.log(`      ↳ 行 ${i + 1}: ${JSON.stringify(a[i])} → ${JSON.stringify(b[i])}`);
        }
    }
}

console.log(failures.length === 0 ? '\n结果：全部 PASS' : `\n结果：${failures.length} 项 FAIL`);
process.exit(failures.length === 0 ? 0 : 1);