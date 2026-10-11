#!/usr/bin/env node
/**
 * [lc-1350] 热补丁器端到端自测（双向验证：未修复必须 FAIL / 修复必须 PASS）。
 * 覆盖：真实安装版 asar 锚点验证、合成 asar 打补丁、幂等重跑、已修复识别、
 *       不兼容中止、restore 卸载、cmd 自解压载荷回环、cmd 冒烟（含自定位模式）。
 * 用法: node patches/hotfix-lc1350/selftest.cjs
 */
'use strict';
const { execFileSync } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const ROOT = path.resolve(__dirname, '..', '..');
const NODE = process.execPath;
const PATCHER = path.join(__dirname, 'patcher.js');
const CMD = path.join(ROOT, 'patches', 'Fntv-Plus热补丁-lc1350-PotPlayer僵尸流修复.cmd');
const ASAR = require.resolve('@electron/asar');
const OVERLAY_REL = 'main/common/playbackShim.js';

const R_OLD_CLOSE = 'if (!responded && !res.writableEnded) {';
const R_NEW_CLOSE = 'if (!res.writableEnded) {';
const R_CB_OPEN = 'requester.request(options, (pres) => {';
const R_GUARD = ' pres.on("error", function () { try { res.destroy(); } catch (e) {} });';

let pass = 0, failN = 0;
function ok(cond, name, detail) {
    if (cond) { pass++; console.log(`  ✓ ${name}${detail ? ' — ' + detail : ''}`); }
    else { failN++; console.log(`  ✗ ${name}${detail ? ' — ' + detail : ''}`); }
}
function runPatcher(args, env) {
    try {
        const out = execFileSync(NODE, [PATCHER, ...args], { encoding: 'utf8', env: { ...process.env, ...env } });
        return { code: 0, out };
    } catch (e) {
        return { code: e.status === undefined ? -1 : e.status, out: (e.stdout || '') + (e.stderr || '') };
    }
}
function runCmd(cmd, args, env, cwd) {
    try {
        const out = execFileSync('cmd.exe', ['/c', cmd, ...args], {
            encoding: 'utf8', env: { ...process.env, ...env }, cwd,
        });
        return { code: 0, out };
    } catch (e) {
        return { code: e.status, out: (e.stdout || '') + (e.stderr || '') };
    }
}

(async () => {
    const work = fs.mkdtempSync(path.join(os.tmpdir(), 'lc1350-selftest-'));
    console.log('工作目录: ' + work);
    const asarMod = require(ASAR);

    // ================= T0: 真实安装版 asar =================
    console.log('\n[T0] 真实安装版 asar 锚点验证');
    const realAsar = path.join(process.env.LOCALAPPDATA || '', 'Programs', 'Fntv-Plus', 'resources', 'app.asar');
    if (fs.existsSync(realAsar)) {
        const p0 = path.join(work, 'p0');
        const r0 = runPatcher(['--asar', realAsar, '--patches', p0]);
        ok(r0.code === 0, '对真实安装版打补丁成功', `exit=${r0.code}`);
        const ov0 = path.join(p0, OVERLAY_REL);
        if (r0.code === 0 && fs.existsSync(ov0)) {
            const patched0 = fs.readFileSync(ov0, 'utf8');
            const orig = require(PATCHER).readAsarFile(realAsar, 'dest/main/common/playbackShim.js').toString('utf8');
            const expected = orig.replace(R_OLD_CLOSE, R_NEW_CLOSE).replace(R_CB_OPEN, R_CB_OPEN + R_GUARD);
            ok(patched0 === expected, '覆盖内容 = 原文件 + 且仅 + 2 处替换');
            ok(!patched0.includes(R_OLD_CLOSE) && patched0.includes(R_NEW_CLOSE), 'close 回收条件已无条件化');
            ok(patched0.includes(R_GUARD), 'pres error 守卫已注入');
            ok(patched0.split(R_GUARD).length - 1 === 1, '守卫仅注入一次');
            console.log('  [T0 信息] 真实安装版是否已含 lc-1265(season_mismatch): ' + orig.includes('season_mismatch'));
        }
    } else {
        console.log('  (本机未安装 Fntv-Plus，跳过真实 asar 用例)');
    }

    // ================= T1: 合成 asar（PRE-fix 编译产物）=================
    console.log('\n[T1] 合成 asar 打补丁 + 幂等');
    const prefixFile = path.join(os.tmpdir(), 'playbackShim.prefix.js');
    const preFixSrc = fs.existsSync(prefixFile)
        ? fs.readFileSync(prefixFile)
        : fs.readFileSync(path.join(ROOT, 'dest', '_verify', 'playbackShim.real380.js'));
    ok(preFixSrc.length > 0, '取得 PRE-fix 源文件', `${preFixSrc.length} bytes`);
    const t1dir = path.join(work, 'src1');
    fs.mkdirSync(path.join(t1dir, 'dest', 'main', 'common'), { recursive: true });
    fs.writeFileSync(path.join(t1dir, 'dest', 'main', 'common', 'playbackShim.js'), preFixSrc);
    fs.writeFileSync(path.join(t1dir, 'package.json'), '{"name":"fntv","main":"dest/main/main.js"}');
    const asar1 = path.join(work, 'test1.asar');
    await asarMod.createPackage(t1dir, asar1);

    const p1dir = path.join(work, 'p1');
    // 版本号标记需要安装签名(exe mtime)：造一个 Fntv-Plus.exe 同名哑文件（patcher 按 basename 门禁）
    const dummyExe = path.join(work, 'Fntv-Plus.exe');
    fs.writeFileSync(dummyExe, 'dummy-exe');
    const cfgOf = (pdir) => JSON.parse(fs.readFileSync(path.join(path.dirname(pdir), 'config.json'), 'utf8'));
    let r1 = runPatcher(['--asar', asar1, '--patches', p1dir, '--exe', dummyExe]);
    ok(r1.code === 0, '打补丁成功', `exit=${r1.code}`);
    const ov1 = path.join(p1dir, OVERLAY_REL);
    ok(fs.existsSync(ov1), 'overlay 已写入');
    ok(fs.existsSync(ov1 + '.lc1350.meta.json'), 'meta 已写入');
    const expected1 = preFixSrc.toString('utf8').replace(R_OLD_CLOSE, R_NEW_CLOSE).replace(R_CB_OPEN, R_CB_OPEN + R_GUARD);
    ok(fs.readFileSync(ov1, 'utf8') === expected1, 'overlay 内容 = PRE-fix + 恰好 2 处替换');
    const cfg1 = cfgOf(p1dir);
    ok(cfg1.appliedPatchVersion === '3.8.0-hotfix', '版本号已标记为 3.8.0-hotfix', String(cfg1.appliedPatchVersion));
    ok(cfg1.appliedPatchSignature === String(fs.statSync(dummyExe).mtimeMs), '安装签名 = exe mtimeMs（与 computeInstallSignature 一致）');

    const bytesBefore = fs.readFileSync(ov1);
    r1 = runPatcher(['--asar', asar1, '--patches', p1dir, '--exe', dummyExe]);
    ok(r1.code === 0 && fs.readFileSync(ov1).equals(bytesBefore), '幂等重跑结果一致');

    // ================= T2: 已修复 asar 识别 =================
    console.log('\n[T2] 已修复版本识别');
    const t2dir = path.join(work, 'src2');
    fs.mkdirSync(path.join(t2dir, 'dest', 'main', 'common'), { recursive: true });
    fs.writeFileSync(path.join(t2dir, 'dest', 'main', 'common', 'playbackShim.js'), expected1);
    const asar2 = path.join(work, 'test2.asar');
    await asarMod.createPackage(t2dir, asar2);
    const p2 = path.join(work, 'p2');
    const r2 = runPatcher(['--asar', asar2, '--patches', p2]);
    ok(r2.code === 0 && r2.out.includes('已是修复版'), '已修复版被识别且直接写覆盖层', `exit=${r2.code}`);
    ok(fs.readFileSync(path.join(p2, OVERLAY_REL), 'utf8') === expected1, '已修复场景 overlay = 原样写入');

    // ================= T3: 不兼容中止 =================
    console.log('\n[T3] 不兼容版本安全中止');
    const t3dir = path.join(work, 'src3');
    fs.mkdirSync(path.join(t3dir, 'dest', 'main', 'common'), { recursive: true });
    fs.writeFileSync(path.join(t3dir, 'dest', 'main', 'common', 'playbackShim.js'), 'console.log("totally different code");\n');
    const asar3 = path.join(work, 'test3.asar');
    await asarMod.createPackage(t3dir, asar3);
    const p3dir = path.join(work, 'p3');
    const r3 = runPatcher(['--asar', asar3, '--patches', p3dir]);
    ok(r3.code === 2, '锚点不符 → exit 2', `exit=${r3.code}`);
    ok(!fs.existsSync(p3dir), '未写入任何覆盖文件');

    // ================= T4: restore 卸载（含版本标记清除与其它补丁保护）=================
    console.log('\n[T4] restore 卸载');
    const r4 = runPatcher(['--restore', '--patches', p1dir]);
    ok(r4.code === 0 && !fs.existsSync(ov1), 'restore 删除 overlay');
    ok(!fs.existsSync(ov1 + '.lc1350.meta.json'), 'restore 删除 meta');
    const cfg4 = cfgOf(p1dir);
    ok(!cfg4.appliedPatchVersion && !cfg4.appliedPatchSignature, 'restore 清除版本号/签名标记');
    const r4b = runPatcher(['--restore', '--patches', p1dir]);
    ok(r4b.code === 0 && r4b.out.includes('没有安装过'), '重复 restore 幂等');
    // 其它补丁保护：版本号不是本补丁时，restore 不得动 overlay 与标记
    fs.writeFileSync(ov1, expected1); // 模拟残留 overlay
    const cfgForeign = cfgOf(p1dir);
    cfgForeign.appliedPatchVersion = '9.9.9-official';
    cfgForeign.appliedPatchSignature = '123.456';
    fs.writeFileSync(path.join(path.dirname(p1dir), 'config.json'), JSON.stringify(cfgForeign, null, 2));
    const r4c = runPatcher(['--restore', '--patches', p1dir]);
    ok(r4c.code === 0 && r4c.out.includes('其它补丁'), '识别出其它补丁并拒绝改动', `exit=${r4c.code}`);
    ok(fs.existsSync(ov1), '其它补丁场景下 overlay 未被删除');
    ok(cfgOf(p1dir).appliedPatchVersion === '9.9.9-official', '其它补丁的版本号标记保留不动');
    fs.rmSync(ov1, { force: true }); // 清场，避免影响后续用例

    // ================= T5: cmd 自解压载荷回环 =================
    console.log('\n[T5] cmd 自解压载荷回环');
    const cmdText = fs.readFileSync(CMD, 'utf8');
    const seg = cmdText.split('__PATCHER_B64_BEGIN__\r\n')[1] || '';
    const b64Block = seg.split('\r\n__PATCHER_B64_END__')[0];
    ok(b64Block.length > 0, 'cmd 内嵌载荷标记存在');
    const decoded = Buffer.from(b64Block.replace(/\r?\n/g, ''), 'base64');
    ok(decoded.equals(fs.readFileSync(PATCHER)), '自解压解码 = patcher.js 原字节');
    ok(b64Block.split('\r\n').every((l) => /^[A-Za-z0-9+/=]*$/.test(l)), '载荷行均为纯 base64');

    // ================= T6: cmd 冒烟（显式安装目录 + 覆盖层重定向）=================
    console.log('\n[T6] cmd 冒烟');
    const t6install = path.join(work, 'install');
    fs.mkdirSync(path.join(t6install, 'resources'), { recursive: true });
    fs.copyFileSync(asar1, path.join(t6install, 'resources', 'app.asar'));
    fs.writeFileSync(path.join(t6install, 'Fntv-Plus.exe'), 'dummy-not-used');
    const p6 = path.join(work, 'p6');
    const env6 = {
        FNTV_ELECTRON_EXE: NODE,
        FNTV_PATCH_DIR_OVERRIDE: p6,
        FNTV_HOTPATCH_NO_PAUSE: '1',
        FNTV_ALLOW_ANY_EXE: '1', // 测试环境 --exe 是 node.exe，放开 basename 门禁
    };
    const c1 = runCmd(CMD, [t6install], env6);
    ok(c1.code === 0, 'cmd 一键打补丁成功', `exit=${c1.code}`);
    ok(fs.existsSync(path.join(p6, OVERLAY_REL)) && fs.readFileSync(path.join(p6, OVERLAY_REL), 'utf8') === expected1, 'cmd 路径 overlay 内容正确');
    const cfg6 = cfgOf(p6);
    ok(cfg6.appliedPatchVersion === '3.8.0-hotfix', 'cmd 路径版本号标记已写入', String(cfg6.appliedPatchVersion));
    ok(cfg6.appliedPatchSignature === String(fs.statSync(NODE).mtimeMs), 'cmd 路径签名 = --exe 的 mtimeMs');
    const c2 = runCmd(CMD, ['restore'], env6);
    ok(c2.code === 0 && !fs.existsSync(path.join(p6, OVERLAY_REL)), 'cmd restore 卸载成功');
    ok(!cfgOf(p6).appliedPatchVersion, 'cmd restore 清除版本号标记');

    // ================= T7: cmd 自定位模式（放入安装根目录双击）=================
    console.log('\n[T7] cmd 自定位模式');
    const p7 = path.join(work, 'p7');
    fs.copyFileSync(CMD, path.join(t6install, 'hotfix.cmd'));
    const c3 = runCmd('hotfix.cmd', [], { ...env6, FNTV_PATCH_DIR_OVERRIDE: p7 }, t6install);
    ok(c3.code === 0 && fs.existsSync(path.join(p7, OVERLAY_REL)), '自定位(%~dp0)模式打补丁成功', `exit=${c3.code}`);
    ok(cfgOf(p7).appliedPatchVersion === '3.8.0-hotfix', '自定位模式版本号标记已写入');

    // ================= T8: 注册表探测路径（无参数、非安装目录运行）=================
    console.log('\n[T8] 注册表探测路径（真实安装版 + 真 Electron 运行时，完整生产路径）');
    if (fs.existsSync(realAsar)) {
        const p8 = path.join(work, 'p8');
        const realExe = path.join(process.env.LOCALAPPDATA, 'Programs', 'Fntv-Plus', 'Fntv-Plus.exe');
        // 不带 FNTV_ELECTRON_EXE：cmd 用注册表找到的安装目录里真实的 Fntv-Plus.exe
        // 以 ELECTRON_RUN_AS_NODE 运行补丁器 —— 与生产完全一致；签名门禁自然通过。
        const env8 = { FNTV_PATCH_DIR_OVERRIDE: p8, FNTV_HOTPATCH_NO_PAUSE: '1' };
        const c4 = runCmd(CMD, [], env8, ROOT); // cwd 在仓库根，不触发 %~dp0
        ok(c4.code === 0, '注册表反推安装目录 → 打补丁成功', `exit=${c4.code}`);
        ok(fs.existsSync(path.join(p8, OVERLAY_REL)), '注册表路径 overlay 已写入');
        const cfg8 = cfgOf(p8);
        ok(cfg8.appliedPatchVersion === '3.8.0-hotfix', '生产路径版本号标记已写入', String(cfg8.appliedPatchVersion));
        ok(cfg8.appliedPatchSignature === String(fs.statSync(realExe).mtimeMs), '生产路径签名 = 真实主 exe 的 mtimeMs');
        if (c4.code === 0) ok(c4.out.includes(process.env.LOCALAPPDATA), '安装目录来自 LOCALAPPDATA\\Programs（注册表或默认路径）');
        const c5 = runCmd(CMD, ['restore'], env8, ROOT);
        ok(c5.code === 0 && !fs.existsSync(path.join(p8, OVERLAY_REL)) && !cfgOf(p8).appliedPatchVersion, '生产路径 restore 完整回滚');
    } else {
        console.log('  (本机未安装 Fntv-Plus，跳过)');
    }

    console.log(`\n===== 结果: ${pass} 通过 / ${failN} 失败 =====`);
    process.exit(failN ? 1 : 0);
})().catch((e) => {
    console.error('自测异常:', e);
    process.exit(1);
});
