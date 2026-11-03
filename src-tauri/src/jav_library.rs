//! JAV 媒体库：番号识别 + 多站点刮削 + 本地索引。
//!
//! 能力移植自 JAVCloudHub 1.9.31（逆向结论见 D:\REA-Work\JAVCloudHub\REPORT.md）：
//!   · 识别（零请求）：文件名/目录名 → 番号（FC2 / 无码 / 有码 / 欧美 / 无番号），规则照抄其 codes.py；
//!     Rust regex 不支持 lookaround，边界一律改写为「前缀捕获 + 非捕获尾巴」（与 cloud_library 同一约定）；
//!   · 扫描分流：cloud_library 扫描时，番号命中的文件改道进本库（见 [`split_candidates`]），
//!     剧集/动漫照旧走 parse_name → TMDB 正常刮削；
//!   · 刮削：r18.dev（公开 JSON，最稳）→ jav321（POST 表单，不吃 CF）→ mgstage / heyzo / FC2（HTML），
//!     按分类取引擎序，首个成功者胜出；封面落本机 library-covers/，经 fntvimg `/__cover/` 协议展示；
//!   · 永远零写入网盘：刮削产物（封面/元数据）只落本机 jav-library.json 与封面目录。
//!
//! 存储：`app_data_dir/jav-library.json`（与 cloud-library.json 同级、同构：每盘一份 entries）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use tauri::AppHandle;

// ───────────────────────── 分类常量（对齐 codes.py） ─────────────────────────

pub const CATEGORY_FC2: &str = "fc2";
pub const CATEGORY_UNCENSORED: &str = "uncensored";
pub const CATEGORY_CENSORED: &str = "censored";
pub const CATEGORY_WESTERN: &str = "western";
pub const CATEGORY_UNKNOWN: &str = "unknown";

pub fn category_label(c: &str) -> &'static str {
    match c {
        CATEGORY_FC2 => "FC2",
        CATEGORY_CENSORED => "有码",
        CATEGORY_UNCENSORED => "无码",
        CATEGORY_WESTERN => "欧美",
        _ => "无番号",
    }
}

/// 是否值得送去刮削（欧美/无番号不进 JAV 库，走正常 TMDB 流程自然落空）
pub fn is_scrapable(category: &str) -> bool {
    matches!(category, CATEGORY_FC2 | CATEGORY_UNCENSORED | CATEGORY_CENSORED)
}

// ───────────────────────── 识别（移植 codes.py） ─────────────────────────

/// 已知片商/系列前缀白名单（照抄 codes.py 的 KNOWN_LABELS 块）。
/// 有码番号只有「标签在白名单里」或「目录名呼应同一番号」才改道 JAV 库，
/// 避免 `OVA-01.mp4` 这类动漫文件被当成番号误伤。
const KNOWN_LABELS: &[&str] = &[
    "SSIS", "SONE", "MIDE", "MIDV", "MIFD", "MIMK", "MISM", "MIUM", "MIZD", "MIAA", "MIAE", "MIGD", "MIRD",
    "IPX", "IPZZ", "IPZ", "IPIT", "IPTD",
    "STARS", "STAR", "START",
    "ABP", "ABW", "ABSD",
    "ADN", "ATOM", "AVOP", "AUKG", "AUKS",
    "BAB", "BABP", "BBAN", "BBTU", "BEAUTY", "BF", "BKD", "BLK", "BLOR", "BOKD", "BOKE", "BOBB", "BONY", "CAWD", "CEMD",
    "CHN", "CJOD", "CLOT", "CRNX", "DASD", "DASS", "DLDSS", "DLN", "DOCP", "DPMI", "DPN", "DVAJ", "DVE", "DVRT",
    "EBOD", "EKDV", "EMRZ", "ENKI", "ESDV", "FCDC", "FERA", "FNS", "FONE", "FS", "FSDSS", "FSET", "FTP", "FTHT",
    "GENM", "GESS", "GLOR", "GNAB", "GVG", "HAKU", "HND", "HNDS", "HODV", "HOMA", "HONB", "HSODA", "HTMS", "IBW",
    "IDBD", "IENE", "IESP", "IESM", "JAC", "JAV", "JJBK", "JJDA", "JUL", "JUR", "JURB",
    "KAAG", "KAM", "KAV", "KBI", "KCD", "KDK", "KEB", "KMHR", "KNMB", "KPB", "KPR", "KRV", "KUSE",
    "LOL", "LULU", "MBDD", "MDBK", "MDD", "MDS", "MDTM", "MEYD", "MFC", "MFYD", "MIA", "MKMP", "MMB", "MMTU", "MOBB", "MOGI", "MOPE", "MPD", "MUD", "MUKD", "MUKO", "MVSD",
    "MXGS", "MXSPS", "NEO", "NHDTB", "NHDTA", "NIKM", "NIN", "NKKD", "NNPJ", "NPG", "NPR", "NTK", "NTR", "NTRD",
    "OFJE", "OKS", "OLM", "OMEG", "ONED", "ONGP", "ONIN", "OPPAI", "OYC", "PARM", "PBD", "PGD", "PKPD", "PPPD",
    "PPPE", "PRED", "PRST", "PSD", "PTS", "PXH", "RBK", "RCTD", "RCT", "REBD", "RKI", "RMMA", "RTP", "SAN", "SANA",
    "SCOP", "SDDE", "SDJS", "SDMF", "SDMU", "SDNM", "SDPT", "SGA", "SHKD", "SHN", "SIRO", "SILK", "SKY", "SNIS",
    "SOE", "SORA", "SPAY", "SSND", "SVDVD", "SW", "TAAK", "TAMO", "TCD",
    "TEK", "TEMU", "TKI", "TMEM", "TMRD", "TNOZ", "TPPN", "TRY", "TSUB", "URE", "UZ", "VAGU", "VEC", "VENU", "VNDS",
    "VRTM", "VSPDS", "WAAA", "WANZ", "WNZ", "WSS", "XRW", "XVSR", "YAG", "YMDD", "YRH", "ZEX", "ZMEN", "ZUKO",
    "MKBD", "MKD", "BGN", "BWBD",
    // [lc-1907c] 实测库补充：白名单覆盖不到的现代厂牌
    "SNOS", "MIDV", "FSDSS", "WAAA", "IPIT", "PRED", "CAWD", "DLDSS",
];

/// 动漫/视频附赠文件里常见的「假番号」前缀：形状和有码番号一样，但绝不是 JAV。
const NON_JAV_LABELS: &[&str] = &["OVA", "CM", "PV", "NCOP", "NCED", "MAD", "OP", "ED", "SP", "MENU", "TRAILER"];

#[derive(Debug, PartialEq, Clone)]
pub struct Detection {
    pub code: String,
    pub category: &'static str,
    /// 分片段号（0 = 未标）
    pub part: u32,
    /// part 是从文件名显式读出来的（-1/_2/part3/CD2），不是排出来的
    pub part_hint: bool,
    pub matched_by: &'static str,
}

struct Res {
    media_ext: regex::Regex,
    bracket: regex::Regex,
    sep: regex::Regex,
    start_sep: regex::Regex,
    spaces: regex::Regex,
    fc2: regex::Regex,
    heyzo: regex::Regex,
    tokyo_hot: regex::Regex,
    tokyo_bare: regex::Regex,
    gachi: regex::Regex,
    laf: regex::Regex,
    unc_num: regex::Regex,
    std_code: regex::Regex,
    west_date: regex::Regex,
    west_hint: regex::Regex,
    tail_part: regex::Regex,
    tail_letter: regex::Regex,
    loose_part: regex::Regex,
    padded5: regex::Regex,
    cup: regex::Regex,
    age: regex::Regex,
    height: regex::Regex,
    date_ymd: regex::Regex,
    field_b: regex::Regex,
    anchor_a: regex::Regex,
    tags_a: regex::Regex,
    noise_span: regex::Regex,
}

static RES: std::sync::OnceLock<Res> = std::sync::OnceLock::new();

fn rx(p: &str) -> regex::Regex {
    regex::Regex::new(p).expect("jav regex")
}

fn res() -> &'static Res {
    RES.get_or_init(|| Res {
        // 双扩展名（.mp4.mp4）逐段剥
        media_ext: rx(r"(?i)\.(mp4|mkv|avi|wmv|mov|ts|m2ts|iso|rmvb|flv|mpg|mpeg|strm|webm|m4v|f4v|vob|3gp|asf|divx|ogm|rm|tp|trp)$"),
        bracket: rx(r"[\[【（(｛{][^\]】）)｝}]*[\]】）)｝}]"),
        sep: rx(r"[\\/|]+"),
        start_sep: rx(r"^[\s\-_.··]+"),
        spaces: rx(r"\s{2,}"),
        // Rust regex 无 lookaround：边界 = 前缀捕获 + 非捕获尾巴（尾巴只影响 end 位置）
        fc2: rx(r"(?:^|[^A-Z0-9])?FC2[\s\-_.]*(?:PPV[\s\-_.]*)?(\d{5,7})(?:$|[^0-9])"),
        heyzo: rx(r"(?:^|[^A-Z0-9])?HEYZO[\s\-_.]*(\d{3,5})(?:$|[^0-9])"),
        tokyo_hot: rx(r"(?:TOKYO[\s\-_.]*HOT|TOKYO-HOT)[\s\-_.]*([NK]?[\s\-_.]?\d{3,5})(?:$|[^0-9])"),
        tokyo_bare: rx(r"(?:^|[^A-Z0-9])TH[\s\-_.]*([NK][\s\-_.]?\d{3,5})(?:$|[^0-9])"),
        gachi: rx(r"(?:^|[^A-Z0-9])?GACHI[\s\-_.]*(\d{3,5})(?:$|[^0-9])"),
        laf: rx(r"(?:^|[^A-Z0-9])?LAF[\s\-_.]*(\d{2,5})(?:$|[^0-9])"),
        unc_num: rx(r"(?:^|[^0-9])(\d{6})[\s\-_.](\d{3})(?:$|[^0-9])"),
        std_code: rx(r"(?:^|[^A-Z0-9])([A-Z]{2,6})[\s\-_.]?(\d{2,5})(?:$|[^0-9])"),
        west_date: rx(r"(?:^|[^0-9])(\d{2})[.\-_](\d{2})[.\-_](\d{2})(?:$|[^0-9])"),
        west_hint: rx(r"XXX|XXP|1080P|2160P|720P|WEB|KTR|RARBG|XXXHD|JAVHD|HEVC|MP4"),
        tail_part: rx(r"^(?:CD|PART|PT|DISC|DISK|VOL|V)?[\s\-_.]*(\d{1,2})$"),
        tail_letter: rx(r"^([A-D])$"),
        loose_part: rx(r"[\s\-_.]+(\d{1,2})$"),
        padded5: rx(r"^([A-Z]{2,6})[\-_.]?00(\d{3})$"),
        cup: rx(r"(?i)(?:^|[^A-Za-z0-9])([A-Za-z])\s*[-－ー]?\s*(?:カップ|CUP|カツプ|罩杯)"),
        age: rx(r"(?:^|[^0-9])(1[89]|[2-5][0-9]|60)\s*(?:歳|才|岁)"),
        height: rx(r"(?:^|[^0-9])(1[3-8][0-9])\s*(?:cm|CM|センチ|公分)"),
        date_ymd: rx(r"(\d{4})[-/.年](\d{1,2})[-/.月](\d{1,2})"),
        // jav321 详情块：`<b>标签</b>: 值<br>`
        field_b: rx(r"(?is)<b>\s*(出演者|出演|ジャンル|類別|メーカー|メーカー名|厂商|片商|配信開始日|発売日|配信日|収録時間)\s*</b>\s*[:：]?\s*(.*?)(?:<br\s*/?>|</p>|$)"),
        anchor_a: rx(r"(?is)<a[^>]*>(.*?)</a>"),
        tags_a: rx(r"(?is)<a[^>]*>"),
        noise_span: rx(r"(?i)\*{2,}[A-Za-z0-9-]+"),
    })
}

/// 去媒体扩展名（容忍 .mp4.mp4 双扩展名，逐段剥）
pub fn strip_media_ext(name: &str) -> String {
    let mut s = name.to_string();
    loop {
        let next = res().media_ext.replace(&s, "").to_string();
        if next == s {
            break;
        }
        s = next;
    }
    s
}

/// 广告前缀 / 括号噪声 / 分隔符清洗，最后统一大写（只影响 ASCII，日文原样）。
pub fn clean_name(raw: &str) -> String {
    let mut s = strip_media_ext(raw);
    // 贪婪删广告前缀：`XXXXX.com@FC2-...` → 取最后一个 @ 之后
    if let Some(i) = s.rfind(['@', '＠']) {
        let ch = s[i..].chars().next().unwrap_or('@');
        s = s[i + ch.len_utf8()..].to_string();
    }
    let s = res().bracket.replace_all(&s, " ");
    let s = s.replace('－', "-");
    let s = res().sep.replace_all(&s, " ");
    let s = res().spaces.replace_all(&s, " ");
    let s = res().start_sep.replace(&s, "");
    s.trim().to_uppercase()
}

/// 匹配结束位置之后找分片段号，返回 (part, 是否显式命中分片规则)。
fn split_tail(tail: &str) -> (u32, bool) {
    let t = tail.trim_matches(|c| matches!(c, ' ' | '-' | '_' | '.' | '·'));
    let r = res();
    if let Some(c) = r.tail_part.captures(t) {
        if let Ok(p) = c[1].parse::<u32>() {
            return (p, true);
        }
    }
    if let Some(c) = r.tail_letter.captures(t) {
        // A~D → 1~4
        let b = c[1].as_bytes()[0];
        return ((b - b'A' + 1) as u32, true);
    }
    if let Some(c) = r.loose_part.captures(t) {
        if let Ok(p) = c[1].parse::<u32>() {
            return (p, false);
        }
    }
    (0, false)
}

fn detect_fc2(cleaned: &str) -> Option<Detection> {
    let r = res();
    let m = r.fc2.captures(cleaned)?;
    let num = m.get(1)?.as_str().to_string();
    let end = m.get(1)?.end();
    let (part, hint) = split_tail(&cleaned[end..]);
    Some(Detection {
        code: format!("FC2-{num}"),
        category: CATEGORY_FC2,
        part,
        part_hint: hint,
        matched_by: "fc2",
    })
}

fn detect_uncensored(cleaned: &str) -> Option<Detection> {
    let r = res();
    let hit = |code: String, cat: &'static str, by: &'static str, end: usize| -> Option<Detection> {
        let (part, hint) = split_tail(&cleaned[end..]);
        Some(Detection { code, category: cat, part, part_hint: hint, matched_by: by })
    };
    if let Some(m) = r.heyzo.captures(cleaned) {
        let g = m.get(1)?;
        return hit(format!("HEYZO-{}", g.as_str()), CATEGORY_UNCENSORED, "heyzo", g.end());
    }
    if let Some(m) = r.tokyo_hot.captures(cleaned) {
        let g = m.get(1)?;
        let inner: String = g.as_str().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        return hit(format!("TH-{inner}"), CATEGORY_UNCENSORED, "tokyohot", g.end());
    }
    if let Some(m) = r.tokyo_bare.captures(cleaned) {
        let g = m.get(1)?;
        let inner: String = g.as_str().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        return hit(format!("TH-{inner}"), CATEGORY_UNCENSORED, "tokyohot", g.end());
    }
    if let Some(m) = r.gachi.captures(cleaned) {
        let g = m.get(1)?;
        return hit(format!("GACHI-{}", g.as_str()), CATEGORY_UNCENSORED, "gachi", g.end());
    }
    if let Some(m) = r.laf.captures(cleaned) {
        let g = m.get(1)?;
        return hit(format!("LAF-{}", g.as_str()), CATEGORY_UNCENSORED, "laf", g.end());
    }
    // 素人无码数字番号（加勒比系 123456-001）：中等置信度
    if let Some(m) = r.unc_num.captures(cleaned) {
        let (a, b) = (m.get(1)?.as_str(), m.get(2)?.as_str());
        let end = m.get(2)?.end();
        return hit(format!("{a}-{b}"), CATEGORY_UNCENSORED, "uncensored-numeric", end);
    }
    None
}

fn detect_censored(cleaned: &str, folder_norm: &str) -> Option<Detection> {
    let r = res();
    // 只认「白名单标签」或「目录名呼应」两种高置信形态；其余一律不认定 ——
    //  OVA-01 这类动漫文件、BLACKEDRAW.19.01.22 这类欧美日期串形状都像番号，放行就是误伤。
    for m in r.std_code.captures_iter(cleaned) {
        let label = m.get(1)?.as_str();
        let num = m.get(2)?.as_str();
        let end = m.get(2)?.end();
        let code = format!("{label}-{num}");
        let (part, hint) = split_tail(&cleaned[end..]);
        if NON_JAV_LABELS.contains(&label) {
            continue;
        }
        // 放宽的几种高置信形态（白名单覆盖不了现实里的全部厂牌）：
        //   ① 标签在白名单            SONE-123.mp4
        //   ② 目录名呼应同一番号        ROYD-123.mp4（目录 ROYD-123 高清）
        //   ③ 番号开头且剩余部分极短    SNOS-332-UC（-UC/-C 这类无码后缀）
        //   ④ 番号开头且紧跟中日文标题   NMSL-024 婚約者に強○され…（JAV 文件名经典形态）
        // 其余（BLACKEDRAW.19.01.22…、BEAUTIFUL GIRL 123）一律不认定。
        let rest = cleaned[end..].trim();
        let rest_is_suffix = {
            let t = rest.trim_start_matches(['-', '_', ' ', '.']);
            !t.is_empty() && t.chars().count() <= 3 && t.chars().all(|c| c.is_ascii_uppercase())
        };
        let rest_starts_cjk = rest
            .chars()
            .next()
            .map(|c| ('\u{4e00}'..='\u{9fff}').contains(&c) || ('\u{3040}'..='\u{30ff}').contains(&c))
            .unwrap_or(false);
        let known = KNOWN_LABELS.contains(&label);
        let echo = !folder_norm.is_empty() && folder_norm.contains(&code.replace('-', ""));
        let bare = m.get(0).map(|g| g.start() == 0).unwrap_or(false)
            && (rest.is_empty() || rest_is_suffix || rest_starts_cjk);
        if known || echo || bare {
            let det = Detection {
                code,
                category: CATEGORY_CENSORED,
                part,
                part_hint: hint,
                matched_by: if known {
                    "std-label"
                } else if echo {
                    "std-echo"
                } else {
                    "std-bare"
                },
            };
            return Some(det);
        }
    }
    None
}

fn detect_western(cleaned: &str) -> Option<Detection> {
    let r = res();
    if r.west_date.is_match(cleaned) || r.west_hint.is_match(cleaned) {
        return Some(Detection {
            code: cleaned.to_string(),
            category: CATEGORY_WESTERN,
            part: 0,
            part_hint: false,
            matched_by: "western-date",
        });
    }
    None
}

/// 从文件名（可附带所在目录名）识别番号。认不出 → None（走正常 TMDB 流程）。
pub fn detect(name: &str, folder: &str) -> Option<Detection> {
    let cleaned = clean_name(name);
    if cleaned.is_empty() {
        return None;
    }
    let folder_norm = normalize_key(folder);
    if let Some(d) = detect_fc2(&cleaned).or_else(|| detect_uncensored(&cleaned)) {
        return Some(d);
    }
    if let Some(d) = detect_censored(&cleaned, &folder_norm) {
        return Some(d);
    }
    detect_western(&cleaned)
}

/// 唯一键：归一化去分隔符，避免 "SONE-123" / "sone_123" 重复入库。
pub fn code_key(code: &str) -> String {
    code.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase()
}

/// 无番号条目的稳定键：U + md5(标题)（对齐 JAVCloudHub 的 _make_unknown_key）。
/// 同一文件反复扫描得到同一个 key，不会凭空多出一堆重复条目。
pub fn unknown_key(title: &str) -> String {
    format!("U{:032x}", md5::compute(title.as_bytes()))
}

fn normalize_key(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase()
}

/// 补零番号的备选：SDMM-00201 → SDMM-201（只在「五位且 00 开头」时摘零）。
pub fn alt_code(code: &str) -> Option<String> {
    let up = code.to_uppercase();
    let c = res().padded5.captures(&up)?;
    Some(format!("{}-{}", &c[1], &c[2]))
}

// ───────────────────────── 身材信息（移植 physique.py，纯本地正则） ─────────────────────────

/// 从若干段文本里抠 {cup, age, height}；先命中的赢（标题里的数字是片方自己标的卖点）。
pub fn extract_physique(texts: &[&str]) -> (String, u32, u32) {
    let r = res();
    let mut cup = String::new();
    let mut age = 0u32;
    let mut height = 0u32;
    for t in texts {
        let t = t.trim();
        if t.is_empty() {
            continue;
        }
        if cup.is_empty() {
            if let Some(c) = r.cup.captures(t) {
                cup = c[1].to_uppercase();
            }
        }
        if age == 0 {
            if let Some(c) = r.age.captures(t) {
                age = c[1].parse().unwrap_or(0);
            }
        }
        if height == 0 {
            if let Some(c) = r.height.captures(t) {
                height = c[1].parse().unwrap_or(0);
            }
        }
    }
    (cup, age, height)
}

/// 站点评分归一（移植 rating.py）：≤5 视为 5 分制 ×2；≤10 原样；>10 当百分制 ÷10。
pub fn rating_to_ten(v: f64) -> f64 {
    if v <= 0.0 {
        return 0.0;
    }
    let out = if v <= 5.0 {
        v * 2.0
    } else if v <= 10.0 {
        v
    } else {
        v / 10.0
    };
    (out * 10.0).round() / 10.0
}

// ───────────────────────── 存储（jav-library.json，与 cloud-library 同构） ─────────────────────────

/// 扫描分流用的原始文件候选（cloud_library 收集，本模块判类）
#[derive(Clone, Debug)]
pub struct JavCandidate {
    pub file_id: String,
    pub parent: String,
    pub name: String,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct JavFile {
    pub provider: String,
    pub file_id: String,
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub part: u32,
}

/// 单条 JAV 条目（一部片一条，分片挂 files）
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct JavEntry {
    pub code: String,
    pub code_key: String,
    pub category: String,
    /// 0 待刮削 | 1 已刮削 | 2 失败
    #[serde(default)]
    pub status: i32,
    #[serde(default)]
    pub title: String,
    /// 预留：AI 汉化标题（后续接 AniBox 既有链路）
    #[serde(default)]
    pub title_zh: String,
    #[serde(default)]
    pub actors: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub release_date: String,
    #[serde(default)]
    pub studio: String,
    #[serde(default)]
    pub series: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub runtime: u32,
    #[serde(default)]
    pub director: String,
    #[serde(default)]
    pub rating: f64,
    #[serde(default)]
    pub votes: u32,
    /// 用户自己打的分（1~10，0=没打）。和站点评分分两列，重刮不抹口味
    #[serde(default)]
    pub user_rating: f64,
    #[serde(default)]
    pub cup: String,
    #[serde(default)]
    pub age: u32,
    #[serde(default)]
    pub height: u32,
    /// library-covers/ 里的封面文件名（经 /__cover/ 协议展示）
    #[serde(default)]
    pub cover: String,
    #[serde(default)]
    pub cover_url: String,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub last_error: String,
    #[serde(default)]
    pub updated_at_ms: u64,
    #[serde(default)]
    pub files: Vec<JavFile>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct JavLib {
    pub provider: String,
    #[serde(default)]
    pub scanned_at_ms: u64,
    #[serde(default)]
    pub entries: Vec<JavEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct JavStore {
    #[serde(default)]
    libs: Vec<JavLib>,
}

fn lib_path() -> Option<PathBuf> {
    crate::cloud::app_data_dir().map(|d| d.join("jav-library.json"))
}

fn load_store() -> JavStore {
    let Some(p) = lib_path() else { return JavStore::default() };
    let Ok(s) = std::fs::read_to_string(p) else { return JavStore::default() };
    serde_json::from_str(&s).unwrap_or_default()
}

fn save_store(st: &JavStore) -> Result<(), String> {
    let Some(p) = lib_path() else { return Err("app_data_dir 未初始化".into()) };
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let s = serde_json::to_string_pretty(st).map_err(|e| format!("序列化 JAV 库失败: {e}"))?;
    std::fs::write(&p, s).map_err(|e| format!("写 JAV 库失败: {e}"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 自然序：数字段按数值比较（"SONE-123-2" 排在 "SONE-123" 之后）
fn natkey(s: &str) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_digit = false;
    for ch in s.chars().chain(std::iter::once('\0')) {
        let d = ch.is_ascii_digit();
        if !cur.is_empty() && d != cur_digit {
            if cur_digit {
                out.push((cur.parse().unwrap_or(u64::MAX), String::new()));
            } else {
                out.push((u64::MAX, cur.to_lowercase()));
            }
            cur.clear();
        }
        cur_digit = d;
        if ch != '\0' {
            cur.push(ch);
        }
    }
    out
}

/// 扫描分流：把候选文件分成（留在正常库的下标）与（改道 JAV 的下标 + 识别结果）。
///
/// 改道门槛（宁缺毋滥）：
///   · FC2 / 无码番号：形状本身就强，直接改道；
///   · 有码番号：标签在白名单里，或目录名呼应同一番号 ——
///     `OVA-01.mp4` 这类动漫文件形状一样但绝不是 JAV，不放行；
///   · 欧美 / 无番号：留在正常库（TMDB 刮不到就显示原名，属「个人视频」自然态）。
pub fn split_candidates(cands: &[JavCandidate]) -> (Vec<usize>, Vec<(usize, Detection)>) {
    let mut normal = Vec::new();
    let mut jav = Vec::new();
    for (i, c) in cands.iter().enumerate() {
        match detect(&c.name, &c.parent) {
            Some(d) if is_scrapable(d.category) => {
                let ok = d.category != CATEGORY_CENSORED || d.matched_by != "std-echo" || {
                    // std-echo 已经要求目录呼应，detect 内部把关；这里恒放行
                    true
                };
                if ok {
                    jav.push((i, d));
                } else {
                    normal.push(i);
                }
            }
            _ => normal.push(i),
        }
    }
    (normal, jav)
}

/// 把改道文件按番号归组、排段号并入库。
///
/// `full=false`（断点续扫）时只合并本批，已有条目原样保留；
/// `full=true`（整库重扫）时以本批为准重建（已刮削的元数据按 code_key 保留）。
pub fn ingest_scan(
    provider: &str,
    cands: &[JavCandidate],
    matched: &[(usize, Detection)],
    replace: bool,
    merge: bool,
) -> Result<usize, String> {
    if matched.is_empty() && replace {
        // 整库重扫但一个番号都没有：清空该盘的 JAV 库
        let mut st = load_store();
        st.libs.retain(|l| l.provider != provider);
        save_store(&st)?;
        return Ok(0);
    }
    if matched.is_empty() {
        return Ok(load_store().libs.iter().find(|l| l.provider == provider).map(|l| l.entries.len()).unwrap_or(0));
    }
    // 分组：code_key → [(candidate, detection)]；合并分片关闭时每个文件自成条目
    //（key 加文件后缀，条目 code 不变但互不合并）
    let mut groups: BTreeMap<String, Vec<(JavCandidate, Detection)>> = BTreeMap::new();
    for (i, d) in matched {
        let Some(c) = cands.get(*i) else { continue };
        let base = if d.category == CATEGORY_UNKNOWN {
            unknown_key(&d.code)
        } else {
            code_key(&d.code)
        };
        let key = if merge { base } else { format!("{}~{}", base, code_key(&c.file_id)) };
        groups.entry(key).or_default().push((c.clone(), d.clone()));
    }

    let mut st = load_store();
    let pos = if let Some(i) = st.libs.iter().position(|l| l.provider == provider) {
        i
    } else {
        st.libs.push(JavLib { provider: provider.to_string(), ..Default::default() });
        st.libs.len() - 1
    };

    // 排段号：显式线索优先，无线索的补最小空闲段号（同批内结果稳定）
    let mut built: Vec<JavEntry> = Vec::new();
    for (key, mut files) in groups {
        files.sort_by(|a, b| natkey(&a.0.name).cmp(&natkey(&b.0.name)));
        let mut used: HashSet<u32> = HashSet::new();
        let mut parts: Vec<u32> = Vec::new();
        for (c, d) in &files {
            if d.part_hint && d.part > 0 && used.insert(d.part) {
                parts.push(d.part);
            } else {
                parts.push(0);
            }
            let _ = c;
        }
        let mut next = 1u32;
        for p in parts.iter_mut() {
            if *p == 0 {
                while !used.insert(next) {
                    next += 1;
                }
                *p = next;
            }
        }
        let jf: Vec<JavFile> = files
            .iter()
            .zip(parts.iter())
            .map(|((c, _), p)| JavFile {
                provider: provider.to_string(),
                file_id: c.file_id.clone(),
                name: c.name.clone(),
                size: c.size,
                part: *p,
            })
            .collect();
        let (code, category) = {
            let d0 = &files[0].1;
            (d0.code.clone(), d0.category)
        };
        let mut entry = JavEntry {
            code,
            code_key: key,
            category: category.to_string(),
            files: jf,
            updated_at_ms: now_ms(),
            ..Default::default()
        };
        // 已有条目：保留刮削元数据，仅更新文件清单
        if let Some(prev) = st.libs[pos].entries.iter().find(|e| e.code_key == entry.code_key) {
            entry.title = prev.title.clone();
            entry.title_zh = prev.title_zh.clone();
            entry.actors = prev.actors.clone();
            entry.tags = prev.tags.clone();
            entry.release_date = prev.release_date.clone();
            entry.studio = prev.studio.clone();
            entry.series = prev.series.clone();
            entry.summary = prev.summary.clone();
            entry.runtime = prev.runtime;
            entry.director = prev.director.clone();
            entry.rating = prev.rating;
            entry.votes = prev.votes;
            entry.cup = prev.cup.clone();
            entry.age = prev.age;
            entry.height = prev.height;
            entry.cover = prev.cover.clone();
            entry.cover_url = prev.cover_url.clone();
            entry.status = prev.status;
            entry.attempts = prev.attempts;
            entry.last_error = prev.last_error.clone();
        }
        entry.files.sort_by_key(|f| f.part);
        built.push(entry);
    }

    if replace {
        st.libs[pos].entries = built;
    } else {
        for e in built {
            match st.libs[pos].entries.iter().position(|x| x.code_key == e.code_key) {
                Some(i) => st.libs[pos].entries[i] = e,
                None => st.libs[pos].entries.push(e),
            }
        }
    }
    let n = st.libs[pos].entries.len();
    save_store(&st)?;
    Ok(n)
}

// ───────────────────────── 刮削引擎 ─────────────────────────

/// 引擎产出的元数据（对齐 scraper/meta.py 的 Meta 字段子集）
#[derive(Default, Debug)]
struct Meta {
    title: String,
    actors: Vec<String>,
    tags: Vec<String>,
    release_date: String,
    studio: String,
    series: String,
    summary: String,
    runtime: u32,
    director: String,
    cover_url: String,
}

// ───────────────────────── 镜像轨（与 bgm/tmdb 镜像同一套准入签名） ─────────────────────────
// r18.dev / jav321 / mgstage / heyzo / FC2 国内直连全挂（DNS 污染 + 阻断），
// JAVCloudHub 原方案靠用户自己的梯子；这里由 jav.690075.xyz（Cloudflare Worker）代拉 ——
// 与 bgm.690075.xyz 治 lain.bgm.tv DNS 污染同一思路。签名协议逐字符对齐 mirror_sign.rs，
// 密钥复用 FNTV_BGM_SECRET（App 注入链路已有，不新增一条；轮换时两个 worker 一起换）。
const MIRROR_BASES: &[&str] = &[
    "https://javmirror.690075.xyz",
    "https://fntv-jav-mirror2.122983191.workers.dev",
];

/// 站点 id → 直连上游（镜像不可用且没配密钥时的兜底，需要用户自己的代理）
const SITE_BASES: &[(&str, &str)] = &[
    ("javbus", "https://www.javbus.com"),
    ("r18", "https://r18.dev"),
    ("j321", "https://www.jav321.com"),
    ("mgs", "https://www.mgstage.com"),
    ("heyzo", "https://www.heyzo.com"),
    ("fc2", "https://adult.contents.fc2.com"),
];

fn site_base(site: &str) -> &'static str {
    SITE_BASES.iter().find(|(k, _)| *k == site).map(|(_, v)| *v).unwrap_or("")
}

fn mirror_secret() -> Option<String> {
    crate::mirror_sign::resolve_secret("FNTV_BGM_SECRET", option_env!("FNTV_BGM_SECRET"))
}

/// 组装一个请求（method/body/业务头；签名头由 apply_auth 在镜像轨上补）
fn build_req(
    cli: &reqwest::Client,
    url: &str,
    method: &str,
    body: Option<&(&'static str, String)>,
    headers: &[(&str, &str)],
) -> reqwest::RequestBuilder {
    let rb = match method {
        "POST" => cli.post(url),
        "PUT" => cli.put(url),
        _ => cli.get(url),
    };
    let rb = if let Some((ct, b)) = body {
        rb.header("content-type", *ct).body(b.clone())
    } else {
        rb
    };
    let mut rb = rb;
    for (k, v) in headers {
        rb = rb.header(*k, *v);
    }
    rb
}

/// r18dev base 覆盖时的直连发送（镜像轨只认内置上游，覆盖域直连）
async fn send_site_with(
    ctx: &Ctx,
    base: String,
    site: &str,
    method: &str,
    path_and_query: &str,
    body: Option<(&'static str, String)>,
    headers: &[(&str, &str)],
) -> Result<reqwest::Response, String> {
    let url = format!("{base}{path_and_query}");
    let rb = build_req(&ctx.client, &url, method, body.as_ref(), headers);
    rb.send().await.map_err(|e| format!("{site} 请求失败: {e}"))
}

/// 站点请求统一入口：有密钥 → 逐镜像端点（边缘代拉，国内直连）；全挂 → 直连上游。
/// 镜像会把上游状态码原样透传，引擎里的 404/403 分支逻辑不变。
async fn send_site(
    ctx: &Ctx,
    site: &str,
    method: &str,
    path_and_query: &str,
    body: Option<(&'static str, String)>,
    headers: &[(&str, &str)],
) -> Result<reqwest::Response, String> {
    let cli = &ctx.client;
    // 用户给这个站填了自定义地址：跳过镜像，直连他的域（填自定义域多半就是为了直连可达）
    let override_base = ctx
        .settings
        .scrape
        .sites
        .iter()
        .find(|s| s.id == site)
        .map(|s| s.base.trim().trim_end_matches('/').to_string())
        .filter(|b| !b.is_empty());
    if let Some(base) = override_base {
        let url = format!("{base}{path_and_query}");
        let rb = build_req(&cli, &url, method, body.as_ref(), headers);
        return rb
            .send()
            .await
            .map_err(|e| format!("{site} 自定义地址直连失败: {e}"));
    }
    let mut blocked: Option<reqwest::Response> = None;
    if let Some(sec) = mirror_secret() {
        for base in MIRROR_BASES {
            let url = format!("{base}/{site}{path_and_query}");
            let pwq = format!("/{site}{path_and_query}");
            let bt = body.as_ref().map(|(_, b)| b.clone()).unwrap_or_default();
            let rb = build_req(&cli, &url, method, body.as_ref(), headers);
            let rb = crate::mirror_sign::apply_auth(rb, &sec, method, &pwq, &bt);
            match rb.send().await {
                // 站点 WAF 挡机房 IP（实测 mgstage/heyzo 403）：先记下，直连有代理时能救回
                Ok(r) if is_blocked_status(r.status().as_u16()) => {
                    if blocked.is_none() {
                        blocked = Some(r);
                    }
                }
                Ok(r) => return Ok(r),
                Err(_) => continue,
            }
        }
    }
    // 直连兜底：镜像不可达，或镜像被站点 WAF 挡且用户自己有代理
    let url = format!("{}{path_and_query}", site_base(site));
    let rb = build_req(&cli, &url, method, body.as_ref(), headers);
    match rb.send().await {
        Ok(r) => return Ok(r),
        Err(e) if blocked.is_none() => {
            return Err(format!("{site} 直连失败（该站点在国内需要代理，或等镜像端点可用）: {e}"))
        }
        Err(_) => {}
    }
    Ok(blocked.expect("blocked response"))
}

/// 封面 URL 能否走镜像的 /img/ 轨（host 在白名单内）
fn img_mirror_path(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    const IMG_HOSTS: &[&str] = &[
        "pics.dmm.co.jp", "p.dmm.co.jp", "c-pics.cdn-jav321.com", "www.mgstage.com",
        "www.heyzo.com", "r18.dev", "adult.contents.fc2.com", "www.jav321.com",
        "www.javbus.com", "pics.javbus.com", "www.javpic.com", "pics.javbus.info",
    ];
    if !IMG_HOSTS.contains(&host) {
        return None;
    }
    Some(format!("/img/{host}/{path}"))
}

/// 一次刮削会话的上下文：客户端按**设置**现建（改代理/UA/超时立即生效），站点表随行
pub struct Ctx {
    pub client: reqwest::Client,
    pub settings: crate::jav_settings::JavSettings,
}

impl Ctx {
    pub fn new(settings: crate::jav_settings::JavSettings) -> Self {
        let n = &settings.network;
        // 代理优先级（借鉴 JAVCloudHub netproxy）：手填 > 环境变量 > Windows 注册表系统代理
        //（Clash/v2rayN 开「系统代理」写的是 WinINET 注册表，不会给本进程塞环境变量）
        let proxy: Option<String> = if !n.proxy.trim().is_empty() {
            Some(normalize_proxy_input(&n.proxy))
        } else if n.trust_env {
            effective_proxy()
        } else {
            None
        };
        let mut b = reqwest::Client::builder()
            .user_agent(if n.user_agent.trim().is_empty() {
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36"
            } else {
                n.user_agent.trim()
            })
            .connect_timeout(std::time::Duration::from_secs(8))
            .timeout(std::time::Duration::from_secs(n.timeout_seconds.clamp(5, 120) as u64))
            .redirect(reqwest::redirect::Policy::limited(5));
        if let Some(p) = proxy {
            if let Ok(px) = reqwest::Proxy::all(&p) {
                b = b.proxy(px);
            }
        }
        Self { client: b.build().expect("jav scrape client"), settings }
    }
}

/// 用户随手填的代理地址补协议头（127.0.0.1:10808 → http://…；本机默认 http，公网默认 https）
fn normalize_proxy_input(raw: &str) -> String {
    let r = raw.trim();
    if r.contains("://") {
        return r.to_string();
    }
    let host = r.split('/').next().unwrap_or(r);
    if host.starts_with("127.") || host.starts_with("localhost") || host.starts_with("[") {
        format!("http://{r}")
    } else {
        format!("https://{r}")
    }
}

/// 环境变量 > Windows 系统代理（注册表 Internet Settings）
fn effective_proxy() -> Option<String> {
    for k in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
        if let Ok(v) = std::env::var(k) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    system_proxy()
}

#[cfg(windows)]
fn system_proxy() -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};

    fn w(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }
    unsafe {
        let sub = w("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings");
        let name = w("ProxyEnable");
        let mut enable: u32 = 0;
        let mut sz = 4u32;
        if RegGetValueW(HKEY_CURRENT_USER, sub.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), &mut enable as *mut u32 as _, &mut sz) != 0 || enable == 0 {
            return None;
        }
        let name = w("ProxyServer");
        let mut buf = [0u16; 512];
        let mut sz = (buf.len() * 2) as u32;
        if RegGetValueW(HKEY_CURRENT_USER, sub.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr() as _, &mut sz) != 0 {
            return None;
        }
        let raw = String::from_utf16_lossy(&buf[..((sz / 2) as usize)]).trim().to_string();
        if raw.is_empty() {
            return None;
        }
        // 形态一："127.0.0.1:7890"；形态二："http=…;https=…"（优先 https）
        let picked = if raw.contains('=') {
            raw.split(';')
                .filter_map(|kv| kv.split_once('='))
                .find(|(k, _)| k.eq_ignore_ascii_case("https"))
                .or_else(|| raw.split(';').filter_map(|kv| kv.split_once('=')).next())
                .map(|(_, v)| v.to_string())?
        } else {
            raw
        };
        if picked.is_empty() {
            return None;
        }
        if picked.contains("://") {
            Some(picked)
        } else {
            Some(format!("http://{picked}"))
        }
    }
}

#[cfg(not(windows))]
fn system_proxy() -> Option<String> {
    None
}

fn is_blocked_status(st: u16) -> bool {
    matches!(st, 403 | 429 | 501 | 503)
}

/// `//cdn...` → `https://cdn...`
fn abs_url(u: &str) -> String {
    let u = u.trim();
    if u.starts_with("//") {
        format!("https:{u}")
    } else {
        u.to_string()
    }
}

fn strip_tags(s: &str) -> String {
    let r = res();
    let no_a = r.anchor_a.replace_all(s, "$1");
    let plain = r.tags_a.replace_all(&no_a, "");
    let plain = plain.replace("</a>", "").replace("</b>", "");
    html_unescape(&plain)
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn norm_date(s: &str) -> String {
    if let Some(c) = res().date_ymd.captures(s) {
        return format!("{}-{:02}-{:02}", &c[1], c[2].parse::<u32>().unwrap_or(0), c[3].parse::<u32>().unwrap_or(0));
    }
    String::new()
}

fn split_people(s: &str) -> Vec<String> {
    s.split([',', '，', '、', '/', '|'])
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

fn uniq_keep_order(v: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for x in v {
        if !x.is_empty() && !out.iter().any(|y| y == &x) {
            out.push(x);
        }
    }
    out
}

// ---------- JavBus：标题中文（繁体）的一手源 —— JAVCloudHub 默认启用它，这就是
// 「原软件标题都是中文」的正主；r18dev/jav321 只有英文/日文。年龄门 over18，走镜像轨。
// 解析选择器照抄 JAVCloudHub scraper_javbus：a.bigImage img / .star-name a /
// span.genre label a / .row.movie .info（發行日期 正则、/studio//series/ 链接）。 ----------

/// 纯解析（可单测）：javbus 详情页 HTML → Meta
fn parse_javbus_html(html: &str) -> Result<Meta, String> {
    let doc = scraper::Html::parse_document(html);
    let first_text = |sel: &str| -> Option<String> {
        let s = scraper::Selector::parse(sel).ok()?;
        doc.select(&s).next().map(|el| {
            el.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
        })
    };
    let all_texts = |sel: &str| -> Vec<String> {
        match scraper::Selector::parse(sel) {
            Ok(s) => doc
                .select(&s)
                .map(|el| el.text().collect::<String>().trim().to_string())
                .filter(|t| !t.is_empty())
                .collect(),
            Err(_) => Vec::new(),
        }
    };
    let first_href_in = |scope: &str, marker: &str| -> String {
        let full = format!("{scope} a[href*='{marker}']");
        let Ok(s) = scraper::Selector::parse(&full) else { return String::new() };
        doc.select(&s).next().map(|el| el.text().collect::<String>().trim().to_string()).unwrap_or_default()
    };
    let raw_title = first_text(".container h3").or_else(|| first_text("h3")).unwrap_or_default();
    // 年龄门/拦截页与「没收录」都当 not-found，让引擎链落到下一个站点
    if raw_title.is_empty() || raw_title.contains("404") || raw_title.contains("搜尋") || raw_title.contains("搜索") {
        return Err("JavBus 没有这个番号（或被年龄门/盾拦下）".into());
    }
    let title = raw_title
        .trim_end_matches(" - JavBus")
        .trim_end_matches(['-', '_', '·', ' '])
        .trim()
        .to_string();
    if title.is_empty() {
        return Err("JavBus 返回页解析不出标题".into());
    }
    let info: String = all_texts(".row.movie .info p").join(" ");
    let runtime = regex::Regex::new(r"(\d+)\s*分鐘")
        .ok()
        .and_then(|r| r.captures(&info))
        .and_then(|c| c[1].parse::<u32>().ok())
        .unwrap_or(0);
    let cover_url = {
        let sel = scraper::Selector::parse("a.bigImage img").ok();
        sel.and_then(|s| doc.select(&s).next())
            .and_then(|el| el.value().attr("src"))
            .map(|u| abs_url(u))
            .unwrap_or_default()
    };
    Ok(Meta {
        title,
        actors: uniq_keep_order(all_texts(".star-name a")),
        tags: uniq_keep_order(all_texts("span.genre label a, .genre-item a")),
        release_date: norm_date(&info),
        studio: first_href_in(".row.movie .info", "/studio/"),
        series: first_href_in(".row.movie .info", "/series/"),
        director: first_href_in(".row.movie .info", "/director/"),
        runtime,
        summary: String::new(),
        cover_url,
    })
}

async fn eng_javbus(ctx: &Ctx, code: &str, base_override: &str) -> Result<Meta, String> {
    let base = if base_override.is_empty() { "https://www.javbus.com" } else { base_override };
    let resp = send_site(
        ctx,
        "javbus",
        "GET",
        &format!("/{code}"),
        None,
        &[
            ("Referer", &format!("{base}/")),
            ("Cookie", "over18=1"),
            ("Accept-Language", "zh-TW,zh;q=0.9"),
        ],
    )
    .await
    .map_err(|e| format!("JavBus 请求失败: {e}"))?;
    let st = resp.status().as_u16();
    if is_blocked_status(st) {
        return Err(format!("JavBus 返回了拦截页（HTTP {st}）"));
    }
    if st == 404 {
        return Err(format!("JavBus 上找不到「{code}」"));
    }
    if !resp.status().is_success() {
        return Err(format!("JavBus 返回 HTTP {st}"));
    }
    let html = resp.text().await.map_err(|e| format!("JavBus 读取失败: {e}"))?;
    parse_javbus_html(&html).map_err(|e| e)
}

// ---------- r18.dev：公开 JSON，最稳的一手（无 Cookie、无年龄门） ----------

async fn eng_r18dev(ctx: &Ctx, code: &str, base_override: &str) -> Result<Meta, String> {
    let host = if base_override.is_empty() { "https://r18.dev" } else { base_override };
    let get = |path: String| {
        let path = path.clone();
        let host = host.to_string();
        let ctx_ref = ctx;
        async move {
            let resp = send_site_with(ctx_ref, host, "r18", "GET", &path, None, &[("Accept", "application/json")])
                .await
                .map_err(|e| format!("R18.dev 请求失败: {e}"))?;
            let st = resp.status().as_u16();
            if st == 404 {
                return Err("notfound".to_string());
            }
            if !resp.status().is_success() {
                return Err(format!("R18.dev 返回 HTTP {st}"));
            }
            let v: Value = resp.json().await.map_err(|_| "R18.dev 返回的不是 JSON".to_string())?;
            Ok(unwrap_movie(v))
        }
    };
    // dvd_id 优先（SONE-116），404 退 combined（部分番号只按 content_id 收录）
    let item = match get(format!("/videos/vod/movies/detail/-/dvd_id={code}/json")).await {
        Ok(v) => v,
        Err(e) if e == "notfound" => get(format!("/videos/vod/movies/detail/-/combined={code}/json")).await?,
        Err(e) => return Err(e),
    };
    if !item.is_object() || item.as_object().map(|o| o.is_empty()).unwrap_or(false) {
        return Err(format!("R18.dev 上找不到「{code}」"));
    }
    let s = |k: &str| item.get(k).and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    let mut title = s("title");
    if title.is_empty() {
        title = s("title_en");
    }
    if title.is_empty() {
        return Err("R18.dev 有返回但解析不出任何字段".into());
    }
    let names = |keys: &[&str], prefer_en: bool| -> Vec<String> {
        for k in keys {
            if let Some(arr) = item.get(*k).and_then(|x| x.as_array()) {
                let mut out = Vec::new();
                for el in arr {
                    let v = if let Some(sv) = el.as_str() {
                        sv.to_string()
                    } else if let Some(o) = el.as_object() {
                        let order: &[&str] = if prefer_en {
                            &["name_en", "name_ja", "name", "name_romaji", "name_kanji"]
                        } else {
                            &["name_ja", "name", "name_kanji", "name_romaji"]
                        };
                        order.iter().find_map(|f| o.get(*f).and_then(|x| x.as_str()).filter(|s| !s.trim().is_empty()).map(|s| s.trim().to_string())).unwrap_or_default()
                    } else {
                        String::new()
                    };
                    if !v.trim().is_empty() {
                        out.push(v.trim().to_string());
                    }
                }
                if !out.is_empty() {
                    return out;
                }
            }
        }
        Vec::new()
    };
    let actors = names(&["actresses", "actress", "actors"], false);
    // 标签优先日文名 —— jav_dict 词典键是日文标签，展示层翻译成简中
    let tags = names(&["categories", "genres"], false);
    let obj_name = |k: &str| -> String {
        match item.get(k) {
            Some(Value::String(s)) => s.trim().to_string(),
            Some(v) => v.get("name").and_then(|x| x.as_str()).unwrap_or("").trim().to_string(),
            None => String::new(),
        }
    };
    let cover_url = item
        .get("images")
        .and_then(|im| im.get("jacket_image"))
        .and_then(|j| ["large2", "large", "thumb"].iter().find_map(|k| j.get(*k).and_then(|x| x.as_str()).filter(|s| !s.is_empty())))
        .unwrap_or("")
        .to_string();
    let runtime = item
        .get("runtime_minutes")
        .or_else(|| item.get("runtime"))
        .and_then(|x| x.as_u64())
        .unwrap_or(0) as u32;
    let director = match item.get("director") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| if let Some(s) = x.as_str() { Some(s.to_string()) } else { x.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()) })
            .collect::<Vec<_>>()
            .join(", "),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    Ok(Meta {
        title,
        actors,
        tags,
        release_date: norm_date(&s("release_date")),
        studio: obj_name("maker"),
        series: obj_name("series"),
        summary: {
            let d = s("description");
            if d.is_empty() { s("summary") } else { d }
        },
        runtime,
        director,
        cover_url: abs_url(&cover_url),
    })
}

/// 返回体可能裸对象、也可能套 {"result": …} / {"data": …}，往下钻一层
fn unwrap_movie(v: Value) -> Value {
    match &v {
        Value::Array(a) => a.first().cloned().unwrap_or(Value::Null),
        Value::Object(o) => {
            for k in ["result", "data"] {
                if let Some(inner) = o.get(k) {
                    if inner.is_object() || inner.is_array() {
                        return unwrap_movie(inner.clone());
                    }
                }
            }
            v
        }
        _ => v,
    }
}

// ---------- jav321：POST 表单检索，不吃 Cloudflare，JavBus/JavDB 被盾时的兜底 ----------

async fn eng_jav321(ctx: &Ctx, code: &str, base_override: &str) -> Result<Meta, String> {
    let base = if base_override.is_empty() { "https://www.jav321.com" } else { base_override };
    let body = format!("sn={}", urlencoding::encode(code));
    let resp = send_site(
        ctx,
        "j321",
        "POST",
        "/search",
        Some(("application/x-www-form-urlencoded", body)),
        &[("Referer", &format!("{base}/")), ("Cookie", "over18=1")],
    )
    .await
    .map_err(|e| format!("jav321 请求失败: {e}"))?;
    let st = resp.status().as_u16();
    if is_blocked_status(st) {
        return Err(format!("jav321 返回了拦截页（HTTP {st}）"));
    }
    if !resp.status().is_success() {
        return Err(format!("jav321 返回 HTTP {st}"));
    }
    let html = resp.text().await.map_err(|e| format!("jav321 读取失败: {e}"))?;
    let doc = scraper::Html::parse_document(&html);
    let first_text = |sel: &str| -> Option<String> {
        let s = scraper::Selector::parse(sel).ok()?;
        doc.select(&s).next().map(|el| {
            el.text()
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
    };
    let title = first_text("div.panel-heading h3").or_else(|| first_text("h3")).unwrap_or_default();
    if title.is_empty() || title.contains("検索結果") || title.contains("Not Found") {
        return Err(format!("jav321 搜不到「{code}」（这个站只覆盖日本有码片）"));
    }
    // 封面走 img 的 src 属性
    let cover_url = scraper::Selector::parse("div.col-md-3 img, div.panel-body img")
        .ok()
        .and_then(|s| doc.select(&s).next())
        .and_then(|el| el.value().attr("src").map(|s| abs_url(s)))
        .unwrap_or_default();

    let mut meta = Meta { title, cover_url, ..Default::default() };
    for c in res().field_b.captures_iter(&html) {
        let key = c[1].to_string();
        let val_raw = c[2].to_string();
        // `<a>名</a> / <a>名</a>` → 纯文本再拆
        let val = {
            let m = &res().anchor_a;
            if m.is_match(&val_raw) {
                m.captures_iter(&val_raw)
                    .map(|x| html_unescape(&x[1]))
                    .collect::<Vec<_>>()
                    .join(" / ")
            } else {
                strip_tags(&val_raw)
            }
        };
        let val = val.trim().to_string();
        if val.is_empty() {
            continue;
        }
        match key.as_str() {
            "出演者" | "出演" => meta.actors = split_people(&val),
            "ジャンル" | "類別" => meta.tags = split_people(&val),
            "メーカー" | "メーカー名" | "厂商" | "片商" => meta.studio = val,
            "配信開始日" | "発売日" | "配信日" => meta.release_date = norm_date(&val),
            "収録時間" => {
                let digits: String = val.chars().filter(|c| c.is_ascii_digit()).collect();
                meta.runtime = digits.parse().unwrap_or(0);
            }
            _ => {}
        }
    }
    Ok(meta)
}

// ---------- mgstage：片商官方站（SOD / MIDE / SIRO / GANA），数据最准但只覆盖自家 ----------

async fn eng_mgstage(ctx: &Ctx, code: &str, base_override: &str) -> Result<Meta, String> {
    let base = if base_override.is_empty() { "https://www.mgstage.com" } else { base_override };
    let resp = send_site(
        ctx,
        "mgs",
        "GET",
        &format!("/product/product_detail/{code}/"),
        None,
        &[("Referer", &format!("{base}/")), ("Cookie", "adc=1")],
    )
    .await
    .map_err(|e| format!("MGStage 请求失败: {e}"))?;
    let st = resp.status().as_u16();
    if st == 404 {
        return Err(format!("MGStage 没有「{code}」这个作品（它只收录自家片商）"));
    }
    if is_blocked_status(st) {
        return Err(format!("MGStage 返回了拦截页（HTTP {st}）"));
    }
    if !resp.status().is_success() {
        return Err(format!("MGStage 返回 HTTP {st}"));
    }
    let html = resp.text().await.map_err(|e| format!("MGStage 读取失败: {e}"))?;
    let doc = scraper::Html::parse_document(&html);
    let text_of = |sel: &str| -> Option<String> {
        let s = scraper::Selector::parse(sel).ok()?;
        doc.select(&s)
            .next()
            .map(|el| el.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" "))
    };
    let title = text_of("h1.tag").or_else(|| text_of("h1")).unwrap_or_default();
    let title = regex::Regex::new(r"^【[^】]*】\s*").unwrap().replace_all(&title, "").trim().to_string();
    if title.is_empty() {
        return Err(format!("MGStage 上没有「{code}」"));
    }
    let cover_url = {
        let s1 = scraper::Selector::parse("a.link_movie img").ok().and_then(|s| doc.select(&s).next().and_then(|el| el.value().attr("src").map(|x| x.to_string())));
        let s2 = scraper::Selector::parse("meta[property='og:image']").ok().and_then(|s| doc.select(&s).next().and_then(|el| el.value().attr("content").map(|x| x.to_string())));
        abs_url(&s1.or(s2).unwrap_or_default())
    };
    let mut meta = Meta { title, cover_url, ..Default::default() };
    // 官方站详情是 th/td 表格，拍平取值
    if let Ok(trs) = scraper::Selector::parse("table.detail tr, table tr") {
        for tr in doc.select(&trs) {
            let th = tr.select(&scraper::Selector::parse("th").unwrap()).next().map(|el| el.text().collect::<String>().trim().trim_end_matches([':', '：']).to_string());
            let Some(th) = th else { continue };
            let td = tr.select(&scraper::Selector::parse("td").unwrap()).next().map(|el| el.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default();
            if td.is_empty() {
                continue;
            }
            match th.as_str() {
                "出演" | "出演者" | "Actor" => meta.actors = split_people(&td),
                "ジャンル" => meta.tags = split_people(&td),
                "メーカー" | "レーベル" | "ブランド" => {
                    if meta.studio.is_empty() {
                        meta.studio = td
                    }
                }
                "シリーズ" => meta.series = td,
                "配信開始日" | "発売日" | "商品発売日" => {
                    if meta.release_date.is_empty() {
                        meta.release_date = norm_date(&td)
                    }
                }
                "収録時間" | "再生時間" => {
                    let digits: String = td.chars().filter(|c| c.is_ascii_digit()).collect();
                    if meta.runtime == 0 {
                        meta.runtime = digits.parse().unwrap_or(0)
                    }
                }
                _ => {}
            }
        }
    }
    Ok(meta)
}

// ---------- heyzo：片商官方站，专治 HEYZO-xxxx 无码片（无 Cookie、无年龄门） ----------

async fn eng_heyzo(ctx: &Ctx, code: &str, _base_override: &str) -> Result<Meta, String> {
    let num = res().heyzo.captures(&code.to_uppercase()).and_then(|c| c.get(1)).map(|g| g.as_str().to_string()).unwrap_or_default();
    if num.is_empty() {
        return Err(format!("「{code}」不是 HEYZO 番号"));
    }
    let resp = send_site(
        ctx,
        "heyzo",
        "GET",
        &format!("/moviepages/{num}/index.html"),
        None,
        &[("Cookie", "adc=1")],
    )
    .await
    .map_err(|e| format!("HEYZO 请求失败: {e}"))?;
    let st = resp.status().as_u16();
    if st == 404 {
        return Err(format!("HEYZO 上没有 {code}"));
    }
    if is_blocked_status(st) {
        return Err(format!("HEYZO 页面被拦了（HTTP {st}）"));
    }
    if !resp.status().is_success() {
        return Err(format!("HEYZO 返回 HTTP {st}"));
    }
    let html = resp.text().await.map_err(|e| format!("HEYZO 读取失败: {e}"))?;
    let doc = scraper::Html::parse_document(&html);
    let text_of = |sel: &str| -> Option<String> {
        let s = scraper::Selector::parse(sel).ok()?;
        doc.select(&s)
            .next()
            .map(|el| el.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" "))
    };
    let meta_attr = |sel: &str, attr: &str| -> Option<String> {
        let s = scraper::Selector::parse(sel).ok()?;
        doc.select(&s).next().and_then(|el| el.value().attr(attr).map(|x| x.to_string()))
    };
    let mut title = text_of("h1").or_else(|| meta_attr("meta[property='og:title']", "content")).unwrap_or_default();
    if title.is_empty() {
        return Err("HEYZO 页面上没找到标题".into());
    }
    let cover_url = abs_url(&meta_attr("meta[property='og:image']", "content").unwrap_or_default());
    let info = scraper::Selector::parse("table.movieInfo")
        .ok()
        .and_then(|s| doc.select(&s).next())
        .map(|el| {
            let raw: String = el.text().collect();
            raw.split_whitespace().collect::<Vec<_>>().join(" ")
        })
        .unwrap_or_default();
    let grab = |re: &str, text: &str| -> String {
        regex::Regex::new(re).ok().and_then(|r| r.captures(text)).map(|c| c[1].trim().to_string()).unwrap_or_default()
    };
    let actors_raw = grab(r"出演\s*(.+?)\s*(?:シリーズ|評価|投稿|女優タイプ|タグ|一括|$)", &info);
    let paren = regex::Regex::new(r"\s*[（(][^)）]*[)）]").unwrap();
    let actors: Vec<String> = actors_raw
        .split(|c: char| c.is_whitespace() || matches!(c, '、' | ',' | '，' | '/'))
        .map(|x| paren.replace_all(x.trim(), "").trim().to_string())
        .filter(|x| !x.is_empty())
        .collect();
    // h1 常写成「标题 - 演员名」，后半截和 actors 重复，切掉
    if let Some(a0) = actors.first() {
        if !a0.is_empty() && title.ends_with(a0.as_str()) {
            if let Some(i) = title.rfind(a0.as_str()) {
                let head = title[..i].trim_end();
                let head = head.trim_end_matches(['-', '–', '—', '|', ' ']);
                if !head.is_empty() {
                    title = head.to_string();
                }
            }
        }
    }
    let date_raw = grab(r"公開日\s*(\d{4}[-/]\d{1,2}[-/]\d{1,2})", &info);
    let tags_raw = grab(r"タグキーワード\s*(.+?)\s*(?:一括|$)", &info);
    let tags: Vec<String> = tags_raw.split(|c: char| c.is_whitespace() || matches!(c, '、' | ',' | '，' | '/')).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
    let series = grab(r"シリーズ\s*(.+?)\s*(?:評価|投稿|女優タイプ|タグ|$)", &info);
    let runtime = regex::Regex::new(r"(\d{1,2}):(\d{2}):(\d{2})")
        .ok()
        .and_then(|r| r.captures(&info))
        .map(|c| c[1].parse::<u32>().unwrap_or(0))
        .unwrap_or(0);
    Ok(Meta {
        title,
        actors,
        tags,
        release_date: norm_date(&date_raw),
        series,
        runtime,
        cover_url,
        ..Default::default()
    })
}

// ---------- FC2 官方站：FC2-PPV 一手来源（日本站，需代理） ----------

async fn eng_fc2(ctx: &Ctx, code: &str, _base_override: &str) -> Result<Meta, String> {
    // 各种写法（FC2-4966160 / FC2-PPV-4966160 / fc2_4966160）都取最长数字段
    let num = code
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>();
    if num.len() < 4 {
        return Err(format!("从「{code}」里提取不出合法的 FC2 作品编号"));
    }
    let resp = send_site(
        ctx,
        "fc2",
        "GET",
        &format!("/article/{num}/"),
        None,
        &[("Referer", "https://adult.contents.fc2.com/"), ("Cookie", "adc=1")],
    )
    .await
    .map_err(|e| format!("FC2 请求失败: {e}"))?;
    let st = resp.status().as_u16();
    if st == 404 {
        return Err("FC2 官方站上没有这个作品（PPV 作品经常下架，下架后官方页是「未找到商品」）".into());
    }
    if is_blocked_status(st) {
        return Err(format!("FC2 返回了拦截页（HTTP {st}，可能需要代理）"));
    }
    if !resp.status().is_success() {
        return Err(format!("FC2 返回 HTTP {st}"));
    }
    let html = resp.text().await.map_err(|e| format!("FC2 读取失败: {e}"))?;
    if html.contains("お探しの商品") || html.contains("見つかりません") {
        return Err("FC2 官方站上没有这个作品（可能已下架或删除）".into());
    }
    let doc = scraper::Html::parse_document(&html);
    let meta_attr = |sel: &str, attr: &str| -> Option<String> {
        let s = scraper::Selector::parse(sel).ok()?;
        doc.select(&s).next().and_then(|el| el.value().attr(attr).map(|x| x.to_string()))
    };
    let text_of = |sel: &str| -> Option<String> {
        let s = scraper::Selector::parse(sel).ok()?;
        doc.select(&s)
            .next()
            .map(|el| el.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" "))
    };
    // 标题：h3 优先（顺手摘掉 FC2 塞进标题里的反爬噪声 `***xjqnsqnjj`），og:title 兜底
    let mut title = text_of(".items_article_headerInfo h3").unwrap_or_default();
    if title.is_empty() {
        title = meta_attr("meta[property='og:title']", "content")
            .or_else(|| meta_attr("meta[name='twitter:title']", "content"))
            .unwrap_or_default();
    }
    title = res().noise_span.replace_all(&title, "").trim().to_string();
    if title.is_empty() {
        return Err("FC2 页面能打开，但解析不出标题（可能改版或触发了风控中间页）".into());
    }
    let cover_url = {
        let s1 = scraper::Selector::parse(".items_article_MainitemThumb img").ok().and_then(|s| doc.select(&s).next().and_then(|el| el.value().attr("src").map(|x| x.to_string())));
        let s2 = meta_attr("meta[property='og:image']", "content").or_else(|| meta_attr("meta[name='twitter:image']", "content"));
        abs_url(&s1.or(s2).unwrap_or_default())
    };
    let mut tags: Vec<String> = scraper::Selector::parse(".items_article_tag a[href*='/tags/'], a.tag.tagTag")
        .ok()
        .map(|s| doc.select(&s).map(|el| el.text().collect::<String>().trim().to_string()).filter(|t| !t.is_empty()).collect())
        .unwrap_or_default();
    tags.push("FC2".into());
    let tags = uniq_keep_order(tags);
    let actors: Vec<String> = scraper::Selector::parse("a[href*='/users/']")
        .ok()
        .map(|s| doc.select(&s).map(|el| el.text().collect::<String>().trim().to_string()).filter(|t| !t.is_empty()).collect())
        .unwrap_or_default();
    let date_raw = text_of(".items_article_Releasedate p, .items_article_softDevice p").unwrap_or_default();
    let summary = meta_attr("meta[property='og:description']", "content").unwrap_or_default();
    Ok(Meta {
        title,
        actors: uniq_keep_order(actors),
        tags,
        release_date: norm_date(&date_raw),
        summary,
        cover_url,
        ..Default::default()
    })
}

/// 某分类下的引擎尝试顺序：来自设置（站点顺序即优先级，停用/地址覆盖随设置）
fn engine_order_ctx(ctx: &Ctx, category: &str) -> Vec<(String, String)> {
    crate::jav_settings::engine_order(&ctx.settings, category)
}

async fn run_engine(ctx: &Ctx, name: &str, code: &str, base: &str) -> Result<Meta, String> {
    match name {
        "javbus" => eng_javbus(ctx, code, base).await,
        "r18dev" => eng_r18dev(ctx, code, base).await,
        "jav321" => eng_jav321(ctx, code, base).await,
        "mgstage" => eng_mgstage(ctx, code, base).await,
        "heyzo" => eng_heyzo(ctx, code, base).await,
        "fc2" => eng_fc2(ctx, code, base).await,
        _ => Err(format!("「{name}」的解析器不存在")),
    }
}

// ───────────────────────── 封面落盘 ─────────────────────────

fn covers_dir() -> Result<PathBuf, String> {
    crate::cloud::app_data_dir()
        .map(|d| d.join("library-covers"))
        .ok_or_else(|| "app_data_dir 未初始化".into())
}

/// 按文件头魔数识别扩展名（不信任 URL / MIME）
fn ext_of(bytes: &[u8], url: &str) -> &'static str {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return "png";
    }
    if bytes.starts_with(&[b'R', b'I', b'F', b'F']) && bytes.len() > 11 && &bytes[8..12] == b"WEBP" {
        return "webp";
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return "jpg";
    }
    let lower = url.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "png"
    } else if lower.ends_with(".webp") {
        "webp"
    } else {
        "jpg"
    }
}

const MAX_COVER_BYTES: usize = 10 * 1024 * 1024;

async fn download_cover(ctx: &Ctx, url: &str, referer: &str) -> Result<(Vec<u8>, String), String> {
    if url.is_empty() {
        return Err("没有封面地址".into());
    }
    let cli = &ctx.client;
    // 镜像 /img/ 轨优先（host 白名单内 + 有密钥）；全挂退直连（需代理）
    let mut last_err = String::new();
    if let (Some(p), Some(sec)) = (img_mirror_path(url), mirror_secret()) {
        for base in MIRROR_BASES {
            let rb = crate::mirror_sign::apply_auth(
                cli.get(format!("{base}{p}")).timeout(std::time::Duration::from_secs(40)),
                &sec,
                "GET",
                &p,
                "",
            );
            match rb.send().await {
                Ok(r) => return finish_cover(r).await,
                Err(e) => last_err = format!("镜像封面下载失败: {e}"),
            }
        }
    }
    let resp = cli
        .get(url)
        .header("Referer", referer)
        .timeout(std::time::Duration::from_secs(40))
        .send()
        .await
        .map_err(|e| if last_err.is_empty() { format!("封面下载失败: {e}") } else { format!("{last_err}；直连失败: {e}") })?;
    finish_cover(resp).await
}

/// 状态码检查（镜像与直连共用）
async fn finish_cover(resp: reqwest::Response) -> Result<(Vec<u8>, String), String> {
    if !resp.status().is_success() {
        return Err(format!("封面下载 HTTP {}", resp.status().as_u16()));
    }
    if let Some(len) = resp.content_length() {
        if len as usize > MAX_COVER_BYTES {
            return Err("封面文件异常大，已跳过".into());
        }
    }
    let bytes = resp.bytes().await.map_err(|e| format!("封面下载失败: {e}"))?;
    if bytes.is_empty() {
        return Err("封面下载为空".into());
    }
    if bytes.len() > MAX_COVER_BYTES {
        return Err("封面文件异常大，已跳过".into());
    }
    let ext = ext_of(&bytes, "");
    Ok((bytes.to_vec(), ext.to_string()))
}

/// 封面写进 library-covers/（内容哈希命名，天然去重），返回文件名。
fn save_cover(_app: &AppHandle, code_key: &str, bytes: &[u8], ext: &str) -> Result<String, String> {
    let dir = covers_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("建封面目录失败: {e}"))?;
    let hash = format!("{:x}", md5::compute(bytes));
    let name = format!("jav-{}-{}.{}", code_key, &hash[..8.min(hash.len())], ext);
    let path = dir.join(&name);
    if !path.exists() {
        let tmp = dir.join(format!("jav-{code_key}-{}.tmp", now_ms()));
        std::fs::write(&tmp, bytes).map_err(|e| format!("写封面失败: {e}"))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("落封面失败: {e}"))?;
    }
    Ok(name)
}

// ───────────────────────── 刮削主流程 ─────────────────────────

/// 单条刮削：按分类引擎序依次降级，首个成功者胜出；封面永远落本机。
async fn scrape_one(ctx: &Ctx, app: &AppHandle, entry: &mut JavEntry, force: bool) -> bool {
    if entry.status == 1 && !force {
        return false;
    }
    if entry.files.is_empty() {
        return false;
    }
    entry.attempts += 1;
    let mut errors: Vec<String> = Vec::new();
    for (eng, base) in engine_order_ctx(ctx, &entry.category) {
        // 补零番号先试原样，再试备选（SDMM-00201 → SDMM-201）
        for code in [entry.code.clone(), alt_code(&entry.code).unwrap_or_default()] {
            if code.is_empty() {
                continue;
            }
            match run_engine(ctx, &eng, &code, &base).await {
                Ok(m) => {
                    if m.title.trim().is_empty() && m.cover_url.is_empty() {
                        errors.push(format!("{eng}: 返回内容为空"));
                        continue;
                    }
                    entry.title = if m.title.is_empty() { entry.code.clone() } else { m.title };
                    if !m.actors.is_empty() {
                        entry.actors = m.actors;
                    }
                    if !m.tags.is_empty() {
                        entry.tags = uniq_keep_order(m.tags);
                    }
                    if !m.release_date.is_empty() {
                        entry.release_date = m.release_date;
                    }
                    if !m.studio.is_empty() {
                        entry.studio = m.studio;
                    }
                    if !m.series.is_empty() {
                        entry.series = m.series;
                    }
                    if !m.summary.is_empty() {
                        entry.summary = m.summary;
                    }
                    if m.runtime > 0 {
                        entry.runtime = m.runtime;
                    }
                    if !m.director.is_empty() {
                        entry.director = m.director;
                    }
                    entry.cover_url = m.cover_url.clone();
                    // 素人片没有演员档案，身材数字只存在于标题/简介里（纯本地正则，零请求）
                    let (cup, age, height) = extract_physique(&[entry.title.as_str(), entry.summary.as_str()]);
                    if !cup.is_empty() {
                        entry.cup = cup;
                    }
                    if age > 0 {
                        entry.age = age;
                    }
                    if height > 0 {
                        entry.height = height;
                    }
                    // 封面下载：第三方图床有防盗链，落本机一次到位（引用站点首页当 Referer）
                    if !m.cover_url.is_empty() {
                        let referer = match eng.as_str() {
                            "fc2" => "https://adult.contents.fc2.com/",
                            "jav321" => "https://www.jav321.com/",
                            "mgstage" => "https://www.mgstage.com/",
                            "heyzo" => "https://www.heyzo.com/",
                            _ => "https://r18.dev/",
                        };
                        match download_cover(ctx, &m.cover_url, referer).await {
                            Ok((bytes, ext)) => match save_cover(app, &entry.code_key, &bytes, &ext) {
                                Ok(name) => entry.cover = name,
                                Err(e) => errors.push(format!("{eng}: 封面落盘失败 {e}")),
                            },
                            Err(e) => errors.push(format!("{eng}: {e}")),
                        }
                    }
                    entry.status = 1;
                    entry.last_error.clear();
                    entry.updated_at_ms = now_ms();
                    // [lc-1907c] 飞牛媒体库来源：把刮削结果同步进本地元数据覆盖层 ——
                    // 飞牛的列表/详情响应会叠加这份元数据（title/简介/日期/评分/封面），
                    // 全程只写本机 metadata_overrides.json，不调飞牛任何写接口。
                    let f0 = entry.files.first().cloned();
                    if let Some(f) = f0 {
                        if f.provider.starts_with("fnos:") && !f.file_id.is_empty() {
                            let choice = crate::metadata_override::MetadataChoice {
                                source: "javhub".into(),
                                id: entry.code_key.clone(),
                                title: if entry.title_zh.trim().is_empty() { entry.title.clone() } else { entry.title_zh.clone() },
                                original_title: entry.title.clone(),
                                year: entry.release_date.get(..4).unwrap_or("").to_string(),
                                date: entry.release_date.clone(),
                                overview: entry.summary.clone(),
                                rating: entry.rating.clamp(0.0, 10.0),
                                poster: if entry.cover.is_empty() { String::new() } else { format!("/__cover/{}", entry.cover) },
                                backdrop: String::new(),
                                seasons: 0,
                                episodes: 0,
                                updated_at: 0,
                            };
                            let _ = crate::metadata_override::metadata_set_override(app.clone(), f.file_id.clone(), choice).await;
                        }
                    }
                    return true;
                }
                Err(e) => errors.push(format!("{eng}: {e}")),
            }
        }
    }
    entry.status = 2;
    entry.last_error = errors.join("；");
    entry.updated_at_ms = now_ms();
    false
}

// ───────────────────────── 命令 ─────────────────────────

/// 单条条目 → 前端形状（coverUrl 走 fntvimg /__cover/ 协议）
fn entry_view(e: &JavEntry) -> Value {
    json!({
        "code": e.code,
        "codeKey": e.code_key,
        "category": e.category,
        "categoryLabel": category_label(&e.category),
        "status": e.status,
        "title": e.title,
        "titleZh": e.title_zh,
        "actors": e.actors,
        "tags": e.tags,
        "releaseDate": e.release_date,
        "studio": e.studio,
        "series": e.series,
        "summary": e.summary,
        "runtime": e.runtime,
        "director": e.director,
        "rating": e.rating,
        "votes": e.votes,
        "cup": e.cup,
        "age": e.age,
        "height": e.height,
        "coverUrl": if e.cover.is_empty() { "".to_string() } else { format!("/__cover/{}", e.cover) },
        "attempts": e.attempts,
        "lastError": e.last_error,
        "files": e.files,
    })
}

/// 某盘的 JAV 条目（含文件清单与封面 URL）
#[tauri::command]
pub async fn jav_library_items(provider: String) -> Result<Value, String> {
    let st = load_store();
    let lib = st.libs.iter().find(|l| l.provider == provider);
    let entries: Vec<Value> = lib
        .map(|l| l.entries.iter().map(entry_view).collect())
        .unwrap_or_default();
    Ok(json!({
        "provider": provider,
        "scannedAtMs": lib.map(|l| l.scanned_at_ms).unwrap_or(0),
        "entries": entries,
    }))
}

/// 全部盘的 JAV 库总览（JavHub 页一次拉全；按 provider 分组）
#[tauri::command]
pub async fn jav_library_all() -> Result<Value, String> {
    let st = load_store();
    let js = crate::jav_settings::load();
    let libs: Vec<Value> = st
        .libs
        .iter()
        .map(|l| {
            // fnos:<媒体库guid> → 显示媒体库标题（tab 不再露一串 guid）
            let label = if let Some(rest) = l.provider.strip_prefix("fnos:") {
                js.fnos.libs.iter().find(|x| x.guid == rest).map(|x| x.title.clone())
            } else if let Some(rest) = l.provider.strip_prefix("local:") {
                // 本地源显示文件夹名
                std::path::Path::new(rest)
                    .file_name()
                    .map(|x| x.to_string_lossy().to_string())
                    .or_else(|| Some(rest.to_string()))
            } else {
                None
            };
            json!({
                "provider": l.provider,
                "label": label,
                "scannedAtMs": l.scanned_at_ms,
                "count": l.entries.len(),
                "entries": l.entries.iter().map(entry_view).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(json!({ "libs": libs }))
}

/// 对某盘的 JAV 条目做番号刮削（增量：status=1 跳过；force=true 重刮全部）。
#[tauri::command]
pub async fn jav_library_scrape(app: AppHandle, provider: String, force: Option<bool>) -> Result<Value, String> {
    let force = force.unwrap_or(false);
    let mut st = load_store();
    let Some(pos) = st.libs.iter().position(|l| l.provider == provider) else {
        return Err("该网盘还没有 JAV 库，请先「加入媒体库」扫描".into());
    };
    let settings = crate::jav_settings::load();
    // 设置里关掉的分类直接不刮（无番号 unknown 本就不在列表里）
    let cats: Vec<&str> = settings.scrape.categories.iter().map(|x| x.as_str()).collect();
    let ctx = Ctx::new(settings.clone());
    let mut scraped = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let cap = 500usize; // 单轮上限：防止几千条的库把一次调用拖到十分钟以上
    let mut done = 0usize;
    let entries = st.libs[pos].entries.clone();
    let mut updated = entries.clone();
    for (i, e) in entries.iter().enumerate() {
        if e.status == 1 && !force {
            skipped += 1;
            continue;
        }
        if !cats.contains(&e.category.as_str()) {
            skipped += 1;
            continue;
        }
        if done >= cap {
            break;
        }
        done += 1;
        let mut cur = e.clone();
        if scrape_one(&ctx, &app, &mut cur, force).await {
            scraped += 1;
        } else if cur.status == 2 {
            failed += 1;
        } else {
            skipped += 1;
        }
        updated[i] = cur;
        // 站点礼貌间隔（设置可调）：批量刮削最怕把第一个站打挂，剩下 2000 条全红
        let gap = (settings.scrape.interval_seconds as f64).clamp(0.2, 60.0);
        tokio::time::sleep(std::time::Duration::from_millis((gap * 1000.0) as u64)).await;
    }
    st.libs[pos].entries = updated;
    st.libs[pos].scanned_at_ms = st.libs[pos].scanned_at_ms.max(now_ms());
    save_store(&st)?;
    Ok(json!({ "scraped": scraped, "failed": failed, "skipped": skipped }))
}

// ───────────────────────── 测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_javbus_detail_page() {
        // [lc-1958] 选择器照抄 JAVCloudHub scraper_javbus；fixture 按真实页面结构缩写
        let html = r#"<!doctype html><html><head><title>SONE-123 真中文標題 - JavBus</title></head><body>
        <div class="container"><h3>SONE-123 真中文標題</h3></div>
        <div class="row movie"><div class="info">
        <p><span class="label">識別碼:</span> SONE-123</p>
        <p><span class="label">發行日期:</span> 2024-01-09</p>
        <p><span class="label">長度:</span> 120分鐘</p>
        <p><span class="label">製作:</span> <a href="https://www.javbus.com/studio/2b1">S1</a></p>
        <p><span class="label">系列:</span> <a href="https://www.javbus.com/series/aaa">中文系列</a></p>
        <span class="genre"><label><a href="/genre/g1">中文字幕</a></label><label><a href="/genre/g2">單體作品</a></label></span>
        </div></div>
        <a class="bigImage" href="/pics/cover/1_b.jpg"><img src="https://www.javbus.com/pics/cover/1_b.jpg" title="第一張"></a>
        <div class="star-name"><a href="/star/abc">女優名</a></div>
        </body></html>"#;
        let m = parse_javbus_html(html).unwrap();
        assert_eq!(m.title, "SONE-123 真中文標題");
        assert_eq!(m.release_date, "2024-01-09");
        assert_eq!(m.runtime, 120);
        assert_eq!(m.studio, "S1");
        assert_eq!(m.series, "中文系列");
        assert!(m.tags.contains(&"中文字幕".to_string()));
        assert_eq!(m.actors, vec!["女優名".to_string()]);
        assert_eq!(m.cover_url, "https://www.javbus.com/pics/cover/1_b.jpg");
    }

    #[test]
    fn parse_javbus_rejects_blocked_or_missing() {
        // 年龄门/拦截页与搜索结果页都不能当详情解析
        assert!(parse_javbus_html("<html><head><title>搜尋結果 - JavBus</title></head><body></body></html>").is_err());
        assert!(parse_javbus_html("<html><head><title>404 Page</title></head><body></body></html>").is_err());
        assert!(parse_javbus_html("<html><head><title>age verify</title></head><body><div class=\"container\"><h3></h3></div></body></html>").is_err());
    }

    #[test]
    fn detect_fc2_variants() {
        for name in ["FC2-PPV-4966160.mp4", "XXXXX.com＠FC2_PPV-4966160.mp4", "fc2_4966160.mkv"] {
            let d = detect(name, "").expect(name);
            assert_eq!(d.category, CATEGORY_FC2, "{name}");
            assert_eq!(d.code, "FC2-4966160", "{name}");
        }
    }

    #[test]
    fn detect_censored_known_label() {
        let d = detect("SONE-123.mp4", "").unwrap();
        assert_eq!((d.code.as_str(), d.category), ("SONE-123", CATEGORY_CENSORED));
        assert_eq!(d.matched_by, "std-label");
    }

    #[test]
    fn detect_censored_bare_code_and_cjk_tail() {
        // [lc-1907c] 放宽后的三种形态：裸番号、短后缀、番号+中日文标题
        assert!(detect("ROYD-123.mp4", "").is_some());
        assert!(detect("SNOS-332-UC.mp4", "").is_some());
        assert!(detect("NMSL-024 婚約者に強○され告訴した女への復讐.mp4", "").is_some());
        // 长英文标题 / 欧美日期串仍然不认定（remainder 太长且非 CJK）
        assert!(detect("BEAUTIFUL GIRL 123.mp4", "").is_none());
        // 欧美日期串：detect 返回 western（由扫描分流拦下，不进 JAV 库）
        assert_eq!(detect("BLACKEDRAW.19.01.22.Bunny.Colby.mp4", "").unwrap().category, CATEGORY_WESTERN);
    }

    #[test]
    fn anime_ova_never_diverts() {
        assert!(detect("OVA-01.mp4", "").is_none());
        assert!(detect("NCOP-02.mp4", "").is_none());
    }

    #[test]
    fn detect_uncensored_variants() {
        let d = detect("HEYZO-1234.mp4", "").unwrap();
        assert_eq!((d.code.as_str(), d.category), ("HEYZO-1234", CATEGORY_UNCENSORED));
        let d = detect("010123-001.mp4", "").unwrap();
        assert_eq!((d.code.as_str(), d.category), ("010123-001", CATEGORY_UNCENSORED));
        let d = detect("Tokyo Hot n0842.mp4", "").unwrap();
        assert_eq!(d.category, CATEGORY_UNCENSORED);
        assert!(d.code.starts_with("TH-"));
    }

    #[test]
    fn part_from_filename() {
        let d = detect("SONE-123-2.mp4", "").unwrap();
        assert_eq!((d.part, d.part_hint), (2, true));
        let d = detect("FC2-PPV-4889513-1.mp4", "").unwrap();
        assert_eq!(d.part, 1);
        let d = detect("SONE-123 CD2.mkv", "").unwrap();
        assert_eq!((d.part, d.part_hint), (2, true));
        let d = detect("SONE-123.mp4", "").unwrap();
        assert_eq!(d.part, 0);
    }

    #[test]
    fn ad_prefix_and_brackets() {
        let d = detect("XXXXX.com@FC2-PPV-4966160【独占】.mp4", "").unwrap();
        assert_eq!(d.code, "FC2-4966160");
    }

    #[test]
    fn western_and_unknown_not_scrapable() {
        let d = detect("BlackedRaw.19.01.22.Bunny.Colby.XXX.1080p.mp4", "").unwrap();
        assert_eq!(d.category, CATEGORY_WESTERN);
        assert!(!is_scrapable(d.category));
        assert!(detect("2024年5月流出，B站清纯反差UP主.mp4", "").is_none());
    }

    #[test]
    fn padded_code_alt() {
        assert_eq!(alt_code("SDMM-00201").as_deref(), Some("SDMM-201"));
        assert_eq!(alt_code("FC2-12345"), None);
        assert_eq!(alt_code("300MIUM-123"), None);
    }

    #[test]
    fn physique_from_title() {
        let (cup, age, height) = extract_physique(&["【18歳】【Gカップ】【身長158cm】整った美少女"]);
        assert_eq!((cup.as_str(), age, height), ("G", 18, 158));
        let (cup, age, height) = extract_physique(&["20才 E cup 165公分"]);
        assert_eq!((cup.as_str(), age, height), ("E", 20, 165));
        // 不带单位的一概不认
        assert_eq!(extract_physique(&["G 158 18"]).0, "");
    }

    #[test]
    fn rating_normalization() {
        assert!((rating_to_ten(4.32) - 8.6).abs() < 1e-9);
        assert!((rating_to_ten(8.6) - 8.6).abs() < 1e-9);
        assert!((rating_to_ten(85.0) - 8.5).abs() < 1e-9);
        assert_eq!(rating_to_ten(0.0), 0.0);
    }

    #[test]
    fn code_key_normalization() {
        assert_eq!(code_key("sone_123"), "SONE123");
        assert_eq!(code_key("FC2-PPV-4966160"), "FC2PPV4966160");
    }

    #[test]
    fn split_candidates_routes() {
        let mk = |id: &str, name: &str, parent: &str| JavCandidate { file_id: id.into(), parent: parent.into(), name: name.into(), size: 1 };
        let cands = vec![
            mk("1", "SONE-123.mp4", "jav"),
            mk("2", "葬送的芙莉莲 S01E24.mp4", "anime"),
            mk("3", "OVA-01.mp4", "anime"),
            mk("4", "FC2-PPV-4966160.mp4", "fc2"),
        ];
        let (normal, jav) = split_candidates(&cands);
        assert_eq!(normal, vec![1, 2]);
        assert_eq!(jav.iter().map(|(_, d)| d.code.as_str()).collect::<Vec<_>>(), vec!["SONE-123", "FC2-4966160"]);
    }
}

// ───────────────────────── 内置 AI：标题汉化 ─────────────────────────

/// 批量把已刮削条目的标题译成目标语言（设置：内置 AI）。
/// 只译 status=1 且 title_zh 为空的条目；单轮上限 30 条（对齐 JAVCloudHub 的 batch_size 默认）。
#[tauri::command]
pub async fn jav_ai_translate(
    app: AppHandle,
    provider: String,
    force: Option<bool>,
    code_key: Option<String>,
) -> Result<Value, String> {
    let js = crate::jav_settings::load();
    if !js.ai.enabled {
        return Err("内置 AI 没有打开（设置 → 内置 AI）".into());
    }
    if js.ai.api_key.trim().is_empty() {
        return Err("没填 API Key（设置 → 内置 AI）".into());
    }
    let force = force.unwrap_or(false);
    let mut st = load_store();
    let Some(pos) = st.libs.iter().position(|l| l.provider == provider) else {
        return Err("该网盘还没有 JAV 库".into());
    };
    // 取待译清单（指定 code_key = 详情抽屉的单条翻译，强制重译）
    let mut todo: Vec<(usize, String, String)> = Vec::new(); // (index, title, actors)
    for (i, e) in st.libs[pos].entries.iter().enumerate() {
        if let Some(ck) = &code_key {
            if &e.code_key != ck {
                continue;
            }
            if e.title.trim().is_empty() {
                continue;
            }
            todo.push((i, e.title.clone(), e.actors.join(", ")));
            break;
        }
        if e.status != 1 || e.title.trim().is_empty() {
            continue;
        }
        if !force && !e.title_zh.trim().is_empty() {
            continue;
        }
        if todo.len() >= 30 {
            break;
        }
        todo.push((i, e.title.clone(), e.actors.join(", ")));
    }
    if todo.is_empty() {
        return Ok(json!({ "translated": 0, "message": "没有待翻译的条目" }));
    }
    // 提示词对齐 JAVCloudHub ai.py：番号与人名保留原字、≤30 字、严格 JSON 出入
    let lang = match js.ai.target_lang.as_str() {
        "zh-TW" => "繁體中文",
        "en" => "English",
        "ja" => "日本語",
        _ => "简体中文",
    };
    let items: Vec<Value> = todo
        .iter()
        .enumerate()
        .map(|(k, (_, t, actors))| json!({ "id": k + 1, "jp": t, "actors": actors }))
        .collect();
    let sys = format!(
        "你是日本成人影片资料库的编辑，任务是把日文标题翻译成简洁自然的{lang}。严格遵守：\n1. 输入是 JSON 数组，每项带 id 和 jp（日文标题）。只输出 JSON：{{\"items\":[{{\"id\":1,\"zh\":\"译名\"}}]}}，不要解释。\n2. 译名控制在 30 个字以内，不加书名号、引号、句号。\n3. 番号（FC2-PPV-123456、SONE-123）和人名（actors 字段里的人名）一律保留日文原字，不要音译。\n4. 输出的 id 必须与输入一一对应，条数一致。直接输出 JSON。"
    );
    let body = json!({
        "model": js.ai.model,
        "temperature": 0.3,
        "response_format": { "type": "json_object" },
        "messages": [ { "role": "system", "content": sys }, { "role": "user", "content": serde_json::to_string(&items).unwrap_or_default() } ],
    });
    let url = format!("{}/chat/completions", js.ai.base_url.trim().trim_end_matches('/'));
    let cli = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|e| format!("HTTP 客户端构建失败: {e}"))?;
    let resp = cli
        .post(&url)
        .bearer_auth(js.ai.api_key.trim())
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("连不上 AI 服务: {e}"))?;
    let st_code = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if st_code != 200 {
        return Err(format!("AI 返回 HTTP {st_code}: {}", text.chars().take(200).collect::<String>()));
    }
    let v: Value = serde_json::from_str(&text).map_err(|_| "AI 返回的不是 JSON".to_string())?;
    let content = v["choices"][0]["message"]["content"].as_str().unwrap_or("").trim().to_string();
    if content.is_empty() {
        return Err("AI 没有返回内容".into());
    }
    // 模型偶尔包 ```json 围栏，抠出来
    let content = content
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim()
        .to_string();
    let parsed: Value = serde_json::from_str(&content).map_err(|e| format!("AI 返回的 JSON 解析失败: {e}"))?;
    let mut translated = 0usize;
    if let Some(items) = parsed["items"].as_array() {
        for it in items {
            let id = it["id"].as_u64().unwrap_or(0) as usize;
            let zh = it["zh"].as_str().unwrap_or("").trim().to_string();
            if id == 0 || zh.is_empty() || id > todo.len() {
                continue;
            }
            let (idx, _, _) = &todo[id - 1];
            st.libs[pos].entries[*idx].title_zh = zh;
            translated += 1;
        }
    }
    save_store(&st)?;
    let _ = &app;
    Ok(json!({ "translated": translated }))
}

// ───────────────────────── 飞牛媒体库接入（设置 → 数据源） ─────────────────────────

/// 飞牛媒体库清单 + 各库的 JAV 刮削开关现状
#[tauri::command]
pub async fn jav_fnos_libs(
    app: AppHandle,
    state: tauri::State<'_, crate::AppState>,
) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    if cfg.token.is_empty() {
        return Err("未登录飞牛".into());
    }
    let client = crate::fnapi::new_client();
    // fnapi::call 已解包 data；mediadb/list 的 data 是**数组**（前端 invoke<Lib[]> 同款），
    // 老版本可能给 {list:[…]}，两种都认
    let v = client
        .call(&cfg.server, "/v/api/v1/mediadb/list", "GET", &cfg.token, None)
        .await
        .map_err(|e| format!("读取媒体库失败: {e}"))?;
    let arr = match v.as_array().cloned() {
        Some(a) => a,
        None => v
            .get("list")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default(),
    };
    let saved = crate::jav_settings::load();
    let mut libs: Vec<Value> = Vec::new();
    for l in &arr {
        let guid = l.get("guid").and_then(|x| x.as_str()).unwrap_or("").to_string();
        if guid.is_empty() {
            continue;
        }
        let title = l
            .get("title")
            .or_else(|| l.get("name"))
            .and_then(|x| x.as_str())
            .unwrap_or("未命名媒体库")
            .to_string();
        let category = l
            .get("category")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let enabled = saved
            .fnos
            .libs
            .iter()
            .find(|x| x.guid == guid)
            .map(|x| x.enabled)
            .unwrap_or(false);
        libs.push(json!({ "guid": guid, "title": title, "category": category, "enabled": enabled }));
    }
    Ok(json!({ "libs": libs }))
}

/// 开/关某个飞牛媒体库的番号刮削
#[tauri::command]
pub async fn jav_fnos_set_lib(app: AppHandle, guid: String, title: String, enabled: bool) -> Result<Value, String> {
    let mut js = crate::jav_settings::load();
    let guid = guid.trim().to_string();
    if guid.is_empty() {
        return Err("缺少媒体库 guid".into());
    }
    if let Some(x) = js.fnos.libs.iter_mut().find(|x| x.guid == guid) {
        x.enabled = enabled;
        if !title.trim().is_empty() {
            x.title = title;
        }
    } else {
        js.fnos.libs.push(crate::jav_settings::FnosLibCfg {
            guid,
            title: if title.trim().is_empty() { "未命名媒体库".into() } else { title },
            enabled,
        });
    }
    crate::jav_settings::save(&js)?;
    Ok(json!({ "ok": true }))
}

/// 扫描启用了番号刮削的飞牛媒体库：**递归下钻文件夹，把里面的视频文件拉出来**。
///
/// 飞牛的 JAV 库结构通常是「库 → 每部片一个文件夹（文件夹名=番号）→ 里面的视频文件」，
/// 顶层 item/list 看到的多是文件夹（类型还常标成 Movie）——只在顶层扫就等于对文件夹刮削。
/// 这里对每个顶层条目下钻（parent_guid + exclude_grouped_video=0），收集真实视频文件；
/// 识别优先级：文件名 → 所在文件夹名（JAV 库的文件夹名基本就是番号）。
/// 用户勾选库 = 全量入库：番号命中的按番号刮削，没命中的以「无番号」入库，
/// 刮削成功的每个视频在 JavHub 单独成卡。
#[tauri::command]
pub async fn jav_fnos_scan(app: AppHandle, state: tauri::State<'_, crate::AppState>) -> Result<Value, String> {
    let cfg = state.config.read().await.clone();
    if cfg.token.is_empty() {
        return Err("未登录飞牛".into());
    }
    let js = crate::jav_settings::load();
    let enabled: Vec<&crate::jav_settings::FnosLibCfg> = js.fnos.libs.iter().filter(|x| x.enabled).collect();
    if enabled.is_empty() {
        return Err("还没有启用任何飞牛媒体库（设置 → 数据源）".into());
    }
    let client = crate::fnapi::new_client();
    const PAGE_SIZE: i64 = 200;
    const MAX_PAGES_PER_DIR: i64 = 10;
    const MAX_DEPTH: usize = 4;
    const MAX_CALLS_PER_LIB: usize = 150; // 防异常大树拖死：每库最多 150 次列目录

    /// 列一个目录的全部条目（自动翻页）。mode: root=ancestor_guid 库根；dir=parent_guid 下钻
    async fn list_dir(
        client: &crate::fnapi::FnClient,
        server: &str,
        token: &str,
        mode: &str,
        guid: &str,
    ) -> Result<Vec<Value>, String> {
        let mut out: Vec<Value> = Vec::new();
        for page in 1..=MAX_PAGES_PER_DIR {
            let body = if mode == "root" {
                json!({
                    "ancestor_guid": guid,
                    "exclude_grouped_video": 1,
                    "sort_type": "ASC", "sort_column": "sort_title",
                    "page_size": PAGE_SIZE, "page": page,
                    "tags": { "type": ["Movie", "TV", "Directory", "Video", "LiveChannel"] }
                })
            } else {
                json!({
                    "parent_guid": guid,
                    "exclude_grouped_video": 0,
                    "sort_type": "ASC", "sort_column": "sort_title",
                    "page_size": PAGE_SIZE, "page": page,
                    "tags": { "type": ["Movie", "TV", "Directory", "Video", "LiveChannel"] }
                })
            };
            let v = client
                .call(server, "/v/api/v1/item/list", "POST", token, Some(body))
                .await?;
            let Some(list) = v.get("list").and_then(|x| x.as_array()) else { break };
            if list.is_empty() {
                break;
            }
            out.extend(list.iter().cloned());
            if (list.len() as i64) < PAGE_SIZE {
                break;
            }
        }
        Ok(out)
    }

    let mut results: Vec<Value> = Vec::new();
    for lib in enabled {
        let mut cands: Vec<JavCandidate> = Vec::new();
        let mut calls = 0usize;
        // 顶层枚举：每个顶层条目都是一个「片子容器」（文件夹），逐个下钻
        let tops = match list_dir(&client, &cfg.server, &cfg.token, "root", &lib.guid).await {
            Ok(v) => v,
            Err(e) => {
                results.push(json!({ "guid": lib.guid, "title": lib.title, "error": format!("读取条目失败: {e}") }));
                continue;
            }
        };
        calls += 1;
        for top in &tops {
            if calls >= MAX_CALLS_PER_LIB {
                break;
            }
            let guid = top.get("guid").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let title = top
                .get("title")
                .or_else(|| top.get("sort_title"))
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            if guid.is_empty() {
                continue;
            }
            // 下钻收集这个容器里的视频文件（BFS，深度受限）
            let mut files: Vec<(String, String)> = Vec::new(); // (guid, 文件名)
            let mut queue: std::collections::VecDeque<(String, String, usize)> =
                std::collections::VecDeque::new();
            queue.push_back((guid.clone(), title.clone(), 0));
            while let Some((fguid, fname, depth)) = queue.pop_front() {
                if calls >= MAX_CALLS_PER_LIB || depth >= MAX_DEPTH {
                    continue;
                }
                calls += 1;
                let Ok(items) = list_dir(&client, &cfg.server, &cfg.token, "dir", &fguid).await else { continue };
                if items.is_empty() {
                    continue;
                }
                for it in &items {
                    let ig = it.get("guid").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let ity = it.get("title").or_else(|| it.get("sort_title")).and_then(|x| x.as_str()).unwrap_or("").to_string();
                    if ig.is_empty() || ity.is_empty() {
                        continue;
                    }
                    let typ = it.get("type").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    if typ.eq_ignore_ascii_case("Directory") {
                        queue.push_back((ig.clone(), ity, depth + 1));
                    } else {
                        files.push((ig, ity));
                    }
                }
            }
            if files.is_empty() {
                // 下钻没拿到文件：顶层条目可能本身就是单视频文件，按原条目收
                files.push((guid, title.clone()));
            }
            // 每个视频文件一个候选；识别用「文件名 + 所在文件夹名」两级
            for (fg, fname) in files {
                cands.push(JavCandidate { file_id: fg, parent: title.clone(), name: fname, size: 0 });
            }
        }
        // 用户明确勾选了这个媒体库 = 库里默认都是 JAV 内容：**只收番号命中的视频**，
        // 没命中的（录屏/游戏录像/无番号杂项）不进 JavHub —— 展示墙只留有海报数据的。
        // 识别优先级：文件名（附文件夹呼应）→ 文件夹名（JAV 库文件夹名=番号）。
        let matched_total = cands.len();
        let mut matched: Vec<(usize, Detection)> = Vec::new();
        for (i, c) in cands.iter().enumerate() {
            if let Some(d) = detect(&c.name, &c.parent)
                .filter(|d| d.category != CATEGORY_WESTERN)
                .or_else(|| detect(&c.parent, "").filter(|d| d.category != CATEGORY_WESTERN))
            {
                matched.push((i, d));
            }
        }
        let entries = ingest_scan(&format!("fnos:{}", lib.guid), &cands, &matched, false, true).unwrap_or(0);
        // 清掉历史遗留的无番号条目（旧版全量入库留下的）：JavHub 只展示有番号的
        let mut st = load_store();
        if let Some(pos) = st.libs.iter().position(|l| l.provider == format!("fnos:{}", lib.guid)) {
            let before = st.libs[pos].entries.len();
            st.libs[pos].entries.retain(|e| e.category != CATEGORY_UNKNOWN);
            if before != st.libs[pos].entries.len() {
                let _ = save_store(&st);
            }
        }
        results.push(json!({
            "guid": lib.guid, "title": lib.title,
            "videos": matched_total, "javDetected": matched.len(), "entries": entries,
        }));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    Ok(json!({ "results": results }))
}

/// 单条刮削（详情抽屉的「重新刮削」）：强制重跑一遍引擎链
#[tauri::command]
pub async fn jav_entry_scrape(app: AppHandle, provider: String, code_key: String) -> Result<Value, String> {
    let settings = crate::jav_settings::load();
    let ctx = Ctx::new(settings.clone());
    let mut st = load_store();
    let Some(pos) = st.libs.iter().position(|l| l.provider == provider) else {
        return Err("该网盘还没有 JAV 库".into());
    };
    let Some(idx) = st.libs[pos].entries.iter().position(|e| e.code_key == code_key) else {
        return Err("条目不存在".into());
    };
    if st.libs[pos].entries[idx].files.is_empty() {
        return Err("该条目没有可播放的文件记录，请先重新扫描".into());
    }
    let mut cur = st.libs[pos].entries[idx].clone();
    scrape_one(&ctx, &app, &mut cur, true).await;
    st.libs[pos].entries[idx] = cur.clone();
    save_store(&st)?;
    Ok(json!({ "ok": cur.status == 1, "entry": entry_view(&cur) }))
}

/// 用户评分（1~10；0 = 清除）。与站点评分分开存，重刮不覆盖。
#[tauri::command]
pub async fn jav_entry_rate(provider: String, code_key: String, value: f64) -> Result<Value, String> {
    let mut st = load_store();
    let Some(pos) = st.libs.iter().position(|l| l.provider == provider) else {
        return Err("该网盘还没有 JAV 库".into());
    };
    let Some(idx) = st.libs[pos].entries.iter().position(|e| e.code_key == code_key) else {
        return Err("条目不存在".into());
    };
    st.libs[pos].entries[idx].user_rating = value.clamp(0.0, 10.0);
    st.libs[pos].entries[idx].updated_at_ms = now_ms();
    save_store(&st)?;
    Ok(json!({ "userRating": st.libs[pos].entries[idx].user_rating }))
}

// ───────────────────────── 本地文件夹数据源 ─────────────────────────

/// 扫描设置里的本地视频文件夹：只读遍历（深度/扩展名/跳过目录走扫描规则设置），
/// 番号命中的入库（provider = "local:<目录绝对路径>"）；没命中的不收（与飞牛口径一致：
/// 海报墙只留有番号数据的）。播放走 jav_local_play 直投 mpv。
#[tauri::command]
pub async fn jav_local_scan(app: AppHandle) -> Result<Value, String> {
    let js = crate::jav_settings::load();
    let dirs = js.local.dirs.clone();
    if dirs.is_empty() {
        return Err("还没有添加本地文件夹（设置 → 数据源）".into());
    }
    let exts: Vec<String> = js
        .scan
        .video_exts
        .iter()
        .map(|x| x.trim_start_matches('.').to_ascii_lowercase())
        .collect();
    let skip: Vec<String> = js.scan.skip_dirs.iter().map(|x| x.to_ascii_lowercase()).collect();
    let prefixes: Vec<String> = js.scan.skip_prefixes.iter().map(|x| x.to_ascii_lowercase()).collect();
    let max_depth = js.scan.max_depth as usize;
    let root_of = |provider: &str| provider.strip_prefix("local:").unwrap_or(provider).to_string();

    let results: Vec<Value> = tauri::async_runtime::spawn_blocking(move || {
        let mut results: Vec<Value> = Vec::new();
        for dir in &dirs {
            let provider = format!("local:{dir}");
            let mut cands: Vec<JavCandidate> = Vec::new();
            let mut stack: Vec<(std::path::PathBuf, usize)> = vec![(std::path::PathBuf::from(dir), 0)];
            while let Some((d, depth)) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&d) else { continue };
                for entry in rd.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let l = name.to_ascii_lowercase();
                    let p = entry.path();
                    if p.is_dir() {
                        if depth < max_depth
                            && !l.starts_with('.')
                            && !skip.iter().any(|x| l == *x)
                            && !prefixes.iter().any(|x| l.starts_with(x.as_str()))
                        {
                            stack.push((p, depth + 1));
                        }
                        continue;
                    }
                    let Some(ext) = p.extension().map(|x| x.to_string_lossy().to_ascii_lowercase()) else { continue };
                    if !exts.contains(&ext) {
                        continue;
                    }
                    let parent = d
                        .file_name()
                        .map(|x| x.to_string_lossy().to_string())
                        .unwrap_or_else(|| dir.clone());
                    cands.push(JavCandidate {
                        file_id: p.to_string_lossy().to_string(),
                        parent,
                        name,
                        size: entry.metadata().map(|m| m.len()).unwrap_or(0),
                    });
                }
            }
            let matched_total = cands.len();
            let (_, matched) = split_candidates(&cands);
            let entries =
                ingest_scan(&provider, &cands, &matched, false, js.scan.merge_multipart).unwrap_or(0);
            results.push(json!({
                "dir": dir, "videos": matched_total,
                "javDetected": matched.len(), "entries": entries,
            }));
        }
        results
    })
    .await
    .map_err(|e| format!("扫描任务失败: {e}"))?;
    let _ = &app;
    Ok(json!({ "results": results }))
}

/// 播放本地视频文件（绝对路径直投 mpv，无需代理/直链解析）
#[tauri::command]
pub async fn jav_local_play(
    app: AppHandle,
    state: tauri::State<'_, crate::AppState>,
    path: String,
) -> Result<(), String> {
    let p = std::path::PathBuf::from(path.trim());
    if !p.is_file() {
        return Err(format!("文件不存在或已移走：{}", p.display()));
    }
    let name = p
        .file_name()
        .map(|x| x.to_string_lossy().to_string())
        .unwrap_or_else(|| "video".into());
    let cfg = state.config.read().await.clone();
    crate::player::launch(&app, &state, &cfg, &p.to_string_lossy(), &name, 0.0, 0.0, None, &[], None, true)
        .await
}
