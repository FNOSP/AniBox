/**
 * [lc-2334] 构建期把 AniBox logo 写进随包 mpv.exe 的图标资源。
 *
 * ── 为什么必须改 exe，而不是运行时设窗口图标 ──────────────────────────────
 * 实测三类事实：
 *   1. mpv 自己不设窗口图标：窗口 `WM_GETICON` / `GWL_HICON` 恒为 0。
 *   2. 它的窗口**类**带图标：`GCLP_HICON` 非 0，指向 exe 资源里的 `IDI_ICON1`
 *      组（RT_ICON 50000~50005），即 mpv 那个深蓝播放图标。
 *   3. Win11 任务栏按钮取类图标 / exe 身份图标；运行期跨进程 `WM_SETICON` 只写
 *      窗口图标槽，任务栏按钮不跟着变 —— 这正是「日志说设图标成功、任务栏仍是
 *      mpv 图标」的真实原因。
 *   （顺带排除的一条路：同进程 WinForms 换图标做对照，任务栏截图通道测不出已有
 *   按钮的图标变化，所以「运行时刷新任务栏」既不可靠也无法验证。）
 *
 * ── 做法：原地覆盖位图字节，不动资源树 ────────────────────────────────────
 * mpv.exe 的 .rsrc 后面还排着别的段，resedit 这类库会直接拒绝
 * （"After Resource section, sections except for relocation are not supported"），
 * 所以直接按 PE 结构定位 RT_ICON 数据、原地覆盖：
 *   · 只改位图数据字节，**不新增/删除任何资源条目** → 资源树结构零变化
 *   · 我们的每张图都比原槽位小（见 tests/analyze_mpv_icon_slots_2334.py），
 *     余下字节补 0，文件长度不变
 *   · 不改段表 / 入口点 / 校验和布局 → 不需要重建资源，也不动签名布局
 *
 * 幂等：写入前比对现有字节，已是我们这张图就跳过。
 *
 * third_party/ 不入库（CI 每次从上游拉包覆盖），所以本脚本在 staging 之后跑。
 */
import fs from "node:fs";
import path from "node:path";
import url from "node:url";

const root = path.resolve(path.dirname(url.fileURLToPath(import.meta.url)), "..");
const MPV_EXE = path.join(root, "third_party", "fntv-mpv", "mpv.exe");
const ICO = path.join(root, "src-tauri", "icons", "icon.ico");

const RT_ICON = 3;
const RT_GROUP_ICON = 14;
const PNG_SIG = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
/** RT_GROUP_ICON 条目固定 14 字节：w,h,colorCount,reserved,planes,bitCount,bytesInRes,nID */
const GROUP_ENTRY_SIZE = 14;

/** 读 .ico → [{ width, height, data }]，按尺寸升序；只接受 PNG 条目。 */
function readIco(file) {
  const buf = fs.readFileSync(file);
  if (buf.readUInt16LE(0) !== 0 || buf.readUInt16LE(2) !== 1) {
    throw new Error(`${file} 不是 .ico`);
  }
  const count = buf.readUInt16LE(4);
  const out = [];
  for (let i = 0; i < count; i++) {
    const o = 6 + i * 16;
    const declW = buf[o] === 0 ? 256 : buf[o];
    const size = buf.readUInt32LE(o + 8);
    const off = buf.readUInt32LE(o + 12);
    const data = Buffer.from(buf.subarray(off, off + size));
    if (!data.subarray(0, PNG_SIG.length).equals(PNG_SIG)) {
      throw new Error(`${file} 里 ${declW}px 条目不是 PNG（本脚本只处理 PNG 格式 ico）`);
    }
    out.push({ width: data.readUInt32BE(16), height: data.readUInt32BE(20), data });
  }
  out.sort((a, b) => a.width - b.width);
  return out;
}

/** 最小 PE 解析：拿资源目录位置 + RVA→文件偏移。 */
function parsePe(buf) {
  const peOff = buf.readUInt32LE(0x3c);
  if (buf.readUInt32LE(peOff) !== 0x00004550) throw new Error("不是 PE 文件");
  const coffOff = peOff + 4;
  const numSections = buf.readUInt16LE(coffOff + 2);
  const optSize = buf.readUInt16LE(coffOff + 16);
  const optOff = coffOff + 20;
  const magic = buf.readUInt16LE(optOff);
  const isPe32Plus = magic === 0x20b;
  if (magic !== 0x10b && !isPe32Plus) throw new Error(`未知 PE 类型 ${magic}`);
  const dataDirOff = optOff + (isPe32Plus ? 112 : 96);
  const rsrcRva = buf.readUInt32LE(dataDirOff + 2 * 8);
  const rsrcSize = buf.readUInt32LE(dataDirOff + 2 * 8 + 4);
  if (!rsrcRva || !rsrcSize) throw new Error("没有资源目录");

  const sections = [];
  const secOff = optOff + optSize;
  for (let i = 0; i < numSections; i++) {
    const o = secOff + i * 40;
    sections.push({
      vsize: buf.readUInt32LE(o + 8),
      va: buf.readUInt32LE(o + 12),
      rawSize: buf.readUInt32LE(o + 16),
      rawPtr: buf.readUInt32LE(o + 20),
    });
  }
  const rvaToOff = (rva) => {
    for (const s of sections) {
      const span = Math.max(s.vsize, s.rawSize);
      if (rva >= s.va && rva < s.va + span) return s.rawPtr + (rva - s.va);
    }
    throw new Error(`RVA 0x${rva.toString(16)} 不在任何段内`);
  };
  return { rsrcRva, rvaToOff };
}

/**
 * 收集 RT_ICON / RT_GROUP_ICON 条目。返回项带 `dataOff`（文件偏移），
 * 图标组额外带 `groupOff`（组目录里那 14 字节记录的偏移），供后续改写长度字段。
 */
function collectResources(buf, pe) {
  const base = pe.rvaToOff(pe.rsrcRva);
  const result = { icons: [], groups: [] };

  const readEntries = (dirOff) => {
    const count = buf.readUInt16LE(dirOff + 12) + buf.readUInt16LE(dirOff + 14);
    const list = [];
    for (let i = 0; i < count; i++) {
      const e = dirOff + 16 + i * 8;
      const nameRaw = buf.readUInt32LE(e);
      const target = buf.readUInt32LE(e + 4);
      list.push({
        id: nameRaw & 0x80000000 ? null : nameRaw & 0x7fffffff,
        isDir: (target & 0x80000000) !== 0,
        offset: base + (target & 0x7fffffff),
      });
    }
    return list;
  };

  for (const typeEntry of readEntries(base)) {
    const type = typeEntry.id;
    if (type !== RT_ICON && type !== RT_GROUP_ICON) continue;
    if (!typeEntry.isDir) continue;
    for (const nameEntry of readEntries(typeEntry.offset)) {
      if (!nameEntry.isDir) continue;
      for (const langEntry of readEntries(nameEntry.offset)) {
        // 叶子层：IMAGE_RESOURCE_DATA_ENTRY
        const de = langEntry.offset;
        const item = {
          id: nameEntry.id, // null = 字符串名
          lang: langEntry.id,
          dataRva: buf.readUInt32LE(de),
          dataSize: buf.readUInt32LE(de + 4),
          dataOff: pe.rvaToOff(buf.readUInt32LE(de)),
        };
        if (type === RT_ICON) {
          result.icons.push(item);
        } else {
          // 组目录第 6 字节起是成员记录数组
          result.groups.push({ ...item, membersOff: item.dataOff + 6 });
        }
      }
    }
  }
  return result;
}

function main() {
  const dryRun = process.argv.includes("--check");

  if (!fs.existsSync(MPV_EXE)) {
    console.error(`[patch-mpv-icon] 找不到 mpv.exe：${MPV_EXE}`);
    process.exit(1);
  }
  if (!fs.existsSync(ICO)) {
    console.error(`[patch-mpv-icon] 找不到图标源：${ICO}`);
    process.exit(1);
  }

  const ico = readIco(ICO);
  const buf = fs.readFileSync(MPV_EXE);
  const pe = parsePe(buf);
  const { icons, groups } = collectResources(buf, pe);

  if (!icons.length) throw new Error("mpv.exe 里没有 RT_ICON 资源");
  if (!groups.length) throw new Error("mpv.exe 里没有 RT_GROUP_ICON 资源");

  const group = groups[0];
  const memberCount = buf.readUInt16LE(group.dataOff + 4);
  console.log(
    `[patch-mpv-icon] 图标组 ${group.id ?? "(字符串名)"} lang=${group.lang}：${memberCount} 个成员`
  );
  console.log(
    `[patch-mpv-icon] 源 ico：${ico.map((e) => `${e.width}x${e.height}`).join(", ")}`
  );

  // 先把替换方案算全，任何一处不满足（找不到档位 / 我们的图更大）就整体失败，
  // 避免写出半套图标这种东西。
  const plan = [];
  for (let i = 0; i < memberCount; i++) {
    const recOff = group.membersOff + i * GROUP_ENTRY_SIZE;
    const w = buf[recOff] === 0 ? 256 : buf[recOff];
    const bytesInRes = buf.readUInt32LE(recOff + 8);
    const memberId = buf.readUInt16LE(recOff + 12);
    const icon = icons.find((r) => r.id === memberId && r.lang === group.lang) ??
      icons.find((r) => r.id === memberId);
    if (!icon) throw new Error(`组成员 ${memberId}（${w}px）找不到对应 RT_ICON`);
    const mine = ico.find((e) => e.width === w);
    if (!mine) throw new Error(`ico 里没有 ${w}px 档，无法覆盖成员 ${memberId}`);
    if (mine.data.length > icon.dataSize) {
      throw new Error(
        `我们的 ${w}px 图标 ${mine.data.length}B 超过槽位 ${icon.dataSize}B，` +
          `无法原地覆盖（需改走重写资源树方案）`
      );
    }
    if (bytesInRes !== icon.dataSize) {
      console.warn(
        `[patch-mpv-icon] 警告：组成员声明的 ${w}px 长度 ${bytesInRes} 与资源实际 ${icon.dataSize} 不一致，以资源实际为准`
      );
    }
    plan.push({ w, memberId, icon, mine, recOff });
  }

  let changed = 0;
  for (const p of plan) {
    const slot = buf.subarray(p.icon.dataOff, p.icon.dataOff + p.icon.dataSize);
    const already =
      slot.subarray(0, p.mine.data.length).equals(p.mine.data) &&
      slot.subarray(p.mine.data.length).every((b) => b === 0);
    if (already) {
      console.log(`[patch-mpv-icon] ${p.w}px（id=${p.memberId}）已是我们的图标`);
      continue;
    }
    p.mine.data.copy(buf, p.icon.dataOff);
    buf.fill(0, p.icon.dataOff + p.mine.data.length, p.icon.dataOff + p.icon.dataSize);
    // 组条目里声明的字节数必须同步：Windows 按这个长度读位图
    buf.writeUInt32LE(p.mine.data.length, p.recOff + 8);
    // 宽高/位深也写实（PNG 的实际尺寸），避免 shell 按旧字段选错档
    buf[p.recOff] = p.mine.width >= 256 ? 0 : p.mine.width;
    buf[p.recOff + 1] = p.mine.height >= 256 ? 0 : p.mine.height;
    buf.writeUInt16LE(32, p.recOff + 6);
    changed++;
    console.log(
      `[patch-mpv-icon] ${p.w}px（id=${p.memberId}）${p.icon.dataSize}B → ${p.mine.data.length}B`
    );
  }

  if (dryRun) {
    console.log(`[patch-mpv-icon] --check：将替换 ${changed} 张位图（未写盘）`);
    return;
  }
  if (!changed) {
    console.log("[patch-mpv-icon] 全部已是我们的图标，无需改动");
    return;
  }

  fs.writeFileSync(MPV_EXE, buf);
  console.log(
    `[patch-mpv-icon] 已写入 ${MPV_EXE}（替换 ${changed} 张，文件大小不变 ${buf.length}B）`
  );
}

try {
  main();
} catch (e) {
  console.error(`[patch-mpv-icon] 失败：${e?.stack || e}`);
  process.exit(1);
}
