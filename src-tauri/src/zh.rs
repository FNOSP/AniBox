//! [title] 展示标题的**繁 → 简**归一化。
//!
//! 背景：飞牛 `play/info` / `episode/list` 返回的剧名、集名来自文件刮削结果，很多
//! 华语动画（尤其台版/港版源）是繁体，而用户要求 mpv 窗口标题固定为简体中文：
//! `剧名 - S1E3: 集名`。
//!
//! 只做**展示层**转换：
//!   · 转换对象 = `playlist_episode_title` 拼出的标题（mpv `force-media-title`、
//!     整季 M3U 的 `#EXTINF`、`write_play_meta` 的 title 展示字段、PotPlayer relay 标题）；
//!   · **不**改 `PlayResolution::tv_title` / `ep_title` 等原始字段，也不动
//!     season/episode/guid/tmdb/bgm 这些匹配锚点 —— 弹幕、片头片尾跳过、进度回传
//!     仍用原值，避免「简繁不一致 → 匹配不到」。
//!
//! 实现：`zhconv` crate（纯 Rust，内嵌 MediaWiki/OpenCC 转换表，编译期生成，
//! 运行时零网络）。只启用 `mediawiki-hans` 一张表（约 0.6 MiB），按字符 + 词组
//! 做 zh-Hans 转换，不做地区词替换（如「計程車」不会变成「出租车」）。
//!
//! 边界：
//!   · 英文/数字/假名/韩文等非中文内容原样保留；
//!   · 已经是简体的标题转换后保持不变（zh-Hans 表对简体是幂等的）；
//!   · 繁体人名（如「愛蜜莉雅」）按字转换（「爱蜜莉雅」），不做音译。

use zhconv::{zhconv, Variant};

/// 是否含 CJK 统一表意文字（含扩展 A/B 及兼容区）。
///
/// 纯英文/假名标题直接跳过转换，省掉首次加载转换表的开销。
fn has_han(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c as u32,
            0x3400..=0x4DBF     // CJK Ext-A
            | 0x4E00..=0x9FFF   // CJK Unified
            | 0xF900..=0xFAFF   // CJK Compatibility
            | 0x20000..=0x2FA1F // CJK Ext-B..F
        )
    })
}

/// 把展示标题转成简体中文。
///
/// 空串返回空串；不含汉字时原样返回；其余走 zh-Hans 转换。
/// 幂等：对已简体的文本调用不会改变内容。
pub fn to_simplified(text: &str) -> String {
    if text.is_empty() || !has_han(text) {
        return text.to_string();
    }
    zhconv(text, Variant::ZhHans)
}

#[cfg(test)]
mod tests {
    use super::to_simplified;

    /// 用户实际遇到的繁体剧名/集名 → 简体。
    #[test]
    fn converts_traditional_title() {
        assert_eq!(
            to_simplified("亂世千金倪亞．利斯頓轉生為嬌弱千金的弒神武人華麗無雙錄"),
            "乱世千金倪亚．利斯顿转生为娇弱千金的弑神武人华丽无双录"
        );
        assert_eq!(to_simplified("舞台落幕之後"), "舞台落幕之后");
    }

    /// 繁体人名：按字转换，不做音译。
    #[test]
    fn converts_traditional_names() {
        assert_eq!(to_simplified("愛蜜莉雅"), "爱蜜莉雅");
        assert_eq!(to_simplified("雷姆與拉姆"), "雷姆与拉姆");
    }

    /// 已是简体：幂等，不产生变化。
    #[test]
    fn keeps_simplified_unchanged() {
        assert_eq!(
            to_simplified("乱世千金倪亚．利斯顿转生为娇弱千金的弑神武人华丽无双录"),
            "乱世千金倪亚．利斯顿转生为娇弱千金的弑神武人华丽无双录"
        );
    }

    /// 非中文（英文/数字/假名/标点）原样保留。
    #[test]
    fn keeps_non_chinese_unchanged() {
        assert_eq!(to_simplified("Re:Zero kara Hajimeru Isekai Seikatsu"), "Re:Zero kara Hajimeru Isekai Seikatsu");
        assert_eq!(to_simplified("S1E3 - Memory Snow"), "S1E3 - Memory Snow");
        assert_eq!(to_simplified("この素晴らしい世界に祝福を！"), "この素晴らしい世界に祝福を！");
        assert_eq!(to_simplified(""), "");
    }

    /// 混合文本：中文部分转换，拉丁字母/数字/假名保持原样。
    #[test]
    fn converts_mixed_text_only_chinese() {
        assert_eq!(to_simplified("Re:Zero 第3話 繁體"), "Re:Zero 第3话 繁体");
        assert_eq!(to_simplified("【繁體】Re:Zero"), "【繁体】Re:Zero");
    }
}
