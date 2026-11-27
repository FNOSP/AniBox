/* [lc-2396] 地区 / 语言 / 制作状态 → 中文映射。
 *
 * 从 `components/detail/MovieHeroEditorial.tsx` 搬出（纯搬移，映射表与拼串逻辑一字未改）：
 * Polaris 详情页（`components/detail/polaris/`）的数据区也要同一套中文标签，
 * 两处各存一份必然漂移（「日本」写成「日本国」这类），所以提到 lib 做**唯一来源**。 */

/** 国家代码 → 中文名（够用即可，未收录的原样显示）。 */
export const COUNTRY_CN: Record<string, string> = {
  JP: "日本",
  CN: "中国",
  HK: "中国香港",
  TW: "中国台湾",
  KR: "韩国",
  US: "美国",
  GB: "英国",
  FR: "法国",
  DE: "德国",
  CA: "加拿大",
  AU: "澳大利亚",
  IN: "印度",
  TH: "泰国",
  SG: "新加坡",
};

/** 语言代码 → 中文名。 */
export const LANG_CN: Record<string, string> = {
  ja: "日语",
  zh: "中文",
  cn: "粤语",
  en: "英语",
  ko: "韩语",
  fr: "法语",
  de: "德语",
  es: "西班牙语",
  th: "泰语",
};

/** 制作状态 → 中文。 */
export const STATUS_CN: Record<string, string> = {
  Released: "已上映",
  "Post Production": "后期制作",
  "In Production": "制作中",
  Planned: "计划中",
  Rumored: "传闻中",
  Canceled: "已取消",
  "Returning Series": "连载中",
  Ended: "已完结",
};

/** 拼「国家 · 语言」展示串；两者都空则返回空串（调用方此时不显示该数据格）。 */
export function regionText(country?: string, language?: string): string {
  const c = (country ?? "").trim().toUpperCase();
  const l = (language ?? "").trim().toLowerCase();
  const cn = c ? COUNTRY_CN[c] ?? c : "";
  const ln = l ? LANG_CN[l] ?? l : "";
  return [cn, ln].filter(Boolean).join(" · ");
}
