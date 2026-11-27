/* [lc-2405] Polaris 详情页 · 文件位置条
 *
 * 取代原「技术规格 4 列玻璃网格 + 展开区」。
 *
 * 用户要求（截图：展开后的「视频」「音频」两个大块）：
 *   「下面的两个大块去掉，长条的展开去掉，保留长条，内容信息改为显示文件位置」
 *   —— 删掉 4 列规格网格与展开区，只留下那条玻璃长条（原来放展开箭头的位置），
 *   长条里的内容改成当前版本的文件位置。
 *
 * 为什么这个删法成立：
 *   · 4 列网格里每一格（分辨率 / 编码 / 位深 / 大小 / 音频 / 容器）在剧集卡上方的
 *     版本卡、以及播放器自己的轨道信息里都能看到；
 *   · 展开区那 12 行里将近一半是「—」（截图可见：帧率 / 基色 / 色彩空间 / 转移特性
 *     全空），把空值排成两栏表格反而稀释了真正有值的那几行；
 *   · 文件位置是**唯一在别处看不到**的信息（排查"这集读的到底哪个文件"时要用），
 *     放进这条本来就在的长条里，位置固定、不额外占高度。
 *
 * 视觉沿用原来那条长条的玻璃板配方（50% 白底 + inset 高光 + 双层投影 + 模糊提饱和），
 * 但**用 div 而不是 button** —— 它不再可点，一个长得像按钮却点不动的元素
 * 比一个普通信息条更容易误导人。
 */
import type { MediaVersion } from "../../../types";

export function PolarisSpecs({
  version,
  filePath,
}: {
  /** 当前选中的视频流（只用于在没给路径时退回文件名；没有则整块不渲染） */
  version: MediaVersion | null;
  /** 当前版本的完整路径（首选） */
  filePath?: string;
}) {
  if (!version) return null;

  /* 首选完整路径；没有路径就退回文件名（至少告诉用户是哪个文件）；
     两者都没有 → 整块不渲染（不留一条空壳）。 */
  const path = String(filePath ?? "").trim();
  const fallback = String(version.file_name ?? "").trim();
  const shown = path || fallback;
  if (!shown) return null;

  return (
    <div className="pdt-filebar" title={shown}>
      <span className="pdt-filebar-k">文件位置</span>
      <span className="pdt-filebar-v">{shown}</span>
    </div>
  );
}
