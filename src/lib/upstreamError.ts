/** [lc-2415] 把后端返回的技术报错转成给用户看的说明。
 *
 * 起因（用户报障原文）：
 *   「所有镜像端点均失败（最后：https://fntv-bgm-mirror.122983191.workers.dev 连接失败/超时:
 *    error sending request for url (.../p1/subjects/837/comments?limit=20&offset=0)）；
 *    降级直连也失败：直连 Bangumi 失败（2 次尝试后）：直连网络错误/超时: error sending
 *    request for url (https://next.bgm.tv/p1/subjects/837/comments?limit=20&offset=0)」
 *
 * 这段话对用户没有任何可操作信息：他既看不懂 reqwest 的 url 报错，也不知道该干什么。
 * 后端把端点名与错误链一条不漏地抛上来是为了排查（日志里保留是对的），
 * 但**界面层**应该只说结论 + 用户能做的事。原始文本仍挂在 title 上，需要排查时能取到。
 *
 * 注意：这里只做"翻译"，不吞错。判断依据是后端稳定的措辞（见 bgm.rs 的错误格式）。
 */

/** 可识别的故障类别 */
export type UpstreamFault =
  | "mirror-down" // 镜像连不上（连接失败/超时）
  | "upstream-5xx" // 镜像可达，但它的上游出错（502/503）
  | "offline" // 本机网络不通
  | "auth" // 镜像签名/密钥问题（我们自己的配置问题）
  | "unknown";

export function classifyUpstreamError(raw: string): UpstreamFault {
  const s = String(raw ?? "");
  if (!s) return "unknown";
  // 签名/密钥类：后端用 auth_error_hint 生成的固定措辞
  if (/bad_signature|stale_signature|bad_app|mirror_misconfigured|镜像拒绝|镜像鉴权失败/.test(s)) {
    return "auth";
  }
  // 上游 5xx：镜像回了状态码（可达），典型 body 是 `error code: 502`
  if (/HTTP 5\d\d|error code: 5\d\d|\b50[234]\b/.test(s)) return "upstream-5xx";
  // 连接层失败
  if (/连接失败|超时|timed? ?out|error sending request|dns|resolve/i.test(s)) {
    return "mirror-down";
  }
  if (/network|网络|offline|ENOTFOUND|ECONNREFUSED/i.test(s)) return "offline";
  return "unknown";
}

/** 给用户的说明文案（标题 + 一句补充）。刻意不出现端点域名与 reqwest 措辞。 */
export function friendlyUpstreamMessage(raw: string): { title: string; hint: string } {
  switch (classifyUpstreamError(raw)) {
    case "upstream-5xx":
      return {
        title: "服务暂时不可用",
        hint: "数据源的接口出错，通常会自行恢复。稍后重试即可。",
      };
    case "mirror-down":
      return {
        title: "连接数据源失败",
        hint: "网络暂时不通，请检查网络后重试。",
      };
    case "offline":
      return {
        title: "当前没有网络",
        hint: "连接恢复后重试即可。",
      };
    case "auth":
      return {
        title: "数据源鉴权失败",
        hint: "这是客户端配置问题，请反馈以便修复。",
      };
    default:
      return {
        title: "加载失败",
        hint: "稍后重试，若持续失败请反馈。",
      };
  }
}
