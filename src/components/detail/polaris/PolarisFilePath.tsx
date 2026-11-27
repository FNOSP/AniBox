/* [lc-2406] Polaris 详情页 · 资源详情行（完整文件路径）
 *
 * 用户给了两张对照图：
 *   图一（我上一版做的）：玻璃长条，标签「文件位置」，内容是**文件名**
 *         → 用户说「文件位置也不对啊」
 *   图二（用户要的）：一行「资源详情　/vol3/1000/番剧/Z/佐佐木与文鸟小哔 (2024)
 *         [tmdbid=206630]/佐佐木与文鸟小哔S2E6第 6 集.mp4　›」
 *         透明底、无边框、标签与路径同一行、**完整路径**、行尾一个 › 指向前进
 *
 * 三条与上一版的实质差别：
 *   1. **显示完整路径**，不是文件名。排查「这集读的到底哪个文件」时文件名不够 ——
 *      同名文件在不同季/不同目录里到处都是，路径才唯一。来源是 `version.file_path`
 *      （lc-2273 加的字段），只有它缺失时才退回文件名。
 *   2. 标签从「文件位置」改成「资源详情」（用户图二的原词，照抄）。
 *   3. 去掉玻璃板：图二是裸文字行，没有半透明白底、没有投影、没有圆角。
 *
 * 断行策略照抄 `MovieHeroEditorial` 的 `.mved-file-path`（lc-2275/2276 已在这台机器上
 * 被用户反复确认过的做法）：按 `/` 拆段、每段 inline-block 整体不可拆、换行只发生在
 * 段间。原因写在那边：`keep-all` / 零宽空格都压不住「字母→CJK」边界的隐式断点，
 * 夹具实测「1080p.|潮」仍会断在段内。段内超长时 `overflow-wrap: anywhere` 兜底。
 *
 * 行尾的 › 是纯装饰（图二里有），不承担交互 —— 这一行整体不可点，
 * 所以不加按钮语义，避免"看起来能点、点了没反应"。
 */
const PATH_SEG_SPLIT = /\//;

export function PolarisFilePath({
  filePath,
  fileName,
}: {
  /** 完整路径（首选） */
  filePath?: string;
  /** 文件名（路径缺失时的兜底） */
  fileName?: string;
}) {
  const path = String(filePath ?? "").trim();
  const shown = path || String(fileName ?? "").trim();
  if (!shown) return null;

  /* 按 `/` 拆段渲染：段整体不可拆，换行只发生在段间。
     拆完补回前导 `/`，保证视觉上仍是完整路径而不是被吃掉分隔符。 */
  const segs = path
    ? path.split(PATH_SEG_SPLIT).filter((s) => s !== "")
    : [];

  return (
    <div className="pdt-filepath-row" title={shown}>
      <span className="pdt-filepath-row-k">资源详情</span>
      <span className="pdt-filepath-row-v">
        {segs.length > 0
          ? segs.map((seg, i) => (
              <span className="pdt-path-seg" key={i}>
                /{seg}
              </span>
            ))
          : shown}
      </span>
      <span className="pdt-filepath-row-chev" aria-hidden="true">
        <svg viewBox="0 0 24 24">
          <polyline
            points="9 6 15 12 9 18"
            fill="none"
            stroke="currentColor"
            strokeWidth="2.4"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      </span>
    </div>
  );
}
