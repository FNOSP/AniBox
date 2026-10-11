//! Authx 签名 —— 与 Fntv-Plus `src/modules/fn_api/request.ts:85-102` 的 genFnAuthx 逐字节对齐。
//! sign 源串 = apiKey_url_nonce_时间戳_dataMd5_apiSecret（"_" 连接后整体 MD5），
//! dataMd5 必须与实际发送的 body 字节完全一致（发送端用同一序列化字符串）。

const API_KEY: &str = "NDzZTVxnRKP8Z0jXg1VAMonaG8akvh";
const API_SECRET: &str = "16CCEB3D-AB42-077D-36A1-F355324E4237";

pub fn md5hex(s: &str) -> String {
    format!("{:x}", md5::compute(s.as_bytes()))
}

/// 6 位随机数字（对齐 request.ts generateRandomDigits(100000, 1000000)）
pub fn rand6() -> String {
    use rand::Rng;
    rand::rng().random_range(100000..1000000).to_string()
}

/// 生成 Authx 头。body 为实际发送的 JSON 字符串；GET 无 body 传 None（md5("")）。
pub fn authx(url_path: &str, body_json: Option<&str>) -> String {
    let nonce = rand6();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let data_md5 = md5hex(body_json.unwrap_or(""));
    let sign = md5hex(&format!(
        "{}_{}_{}_{}_{}_{}",
        API_KEY, url_path, nonce, timestamp, data_md5, API_SECRET
    ));
    format!("nonce={}&timestamp={}&sign={}", nonce, timestamp, sign)
}
