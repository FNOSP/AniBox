//! 飞牛影视 API 客户端。对齐 Fntv-Plus `src/modules/fn_api/request.ts`：
//! - POST/PUT 在 body 末尾追加 nonce 防重放（签名对最终 body 字节计算）
//! - 鉴权双通道：Authorization 头 + Cookie: mode=relay; Trim-MC-token=<token>
//! - 3xx 手动跟随（重签新路径）；code==5000 invalid sign 重试；code!=0 视为业务错误
//! - 自签证书：内网 NAS 预期自签，PoC 直接 accept_invalid_certs（对齐 lc-286 私有地址免验）

use serde_json::{json, Value};

pub struct FnClient {
    http: reqwest::Client,
}

/// 服务器地址归一化：宽容处理用户输入（思路对齐主仓库 auth.ts）——
/// 全角冒号转半角、去引号/空白、无协议头自动补 http://、去掉端口后的路径。
/// 用户输入 `100.66.1.2`（裸 IP）即可用，等价 http://100.66.1.2（默认 80 端口）。
pub fn normalize_server(input: &str) -> String {
    let mut s = input.trim().to_string();
    s = s.replace('：', ":");
    s = s
        .trim_matches(|c: char| c == '"' || c == '\'' || c == ' ')
        .to_string();
    let lower = s.to_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        s = format!("http://{}", s);
    }
    if let Some(idx) = s.find("://") {
        let rest = &s[idx + 3..];
        if let Some(slash) = rest.find('/') {
            s = format!("{}//{}", &s[..idx + 3], &rest[..slash]);
        }
    }
    s
}

pub fn new_client() -> FnClient {
    let http = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(8))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .expect("reqwest client build");
    FnClient { http }
}

/// 拆绝对 Location 为 (base, path)；相对 Location 返回 (base, location)
fn split_redirect(base: &str, location: &str) -> (String, String) {
    if let Some(idx) = location.find("://") {
        let after = &location[idx + 3..];
        match after.find('/') {
            Some(slash) => (format!("{}//{}", &location[..idx + 3], &after[..slash]), after[slash..].to_string()),
            None => (location.to_string(), "/".to_string()),
        }
    } else {
        (base.to_string(), location.to_string())
    }
}

impl FnClient {
    pub async fn call(
        &self,
        base: &str,
        path: &str,
        method: &str,
        token: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        let base = normalize_server(base);
        self.call_inner(&base, path, method, token, body, 0).await
    }

    fn call_inner<'a>(
        &'a self,
        base: &'a str,
        path: &'a str,
        method: &'a str,
        token: &'a str,
        body: Option<Value>,
        depth: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, String>> + Send + 'a>>
    {
        Box::pin(async move {
        if depth > 3 {
            return Err("重定向次数过多".into());
        }        // POST/PUT 追加 nonce 并以最终序列化字符串为签名依据（字节级一致，见模块注释）
        let is_post = method.eq_ignore_ascii_case("post");
        let body_json: String = if is_post {
            let mut obj = body
                .as_ref()
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            obj.insert("nonce".into(), json!(crate::authx::rand6()));
            Value::Object(obj).to_string()
        } else {
            String::new()
        };

        let authx = crate::authx::authx(path, if is_post { Some(body_json.as_str()) } else { None });
        let cookie = if token.is_empty() {
            "mode=relay".to_string()
        } else {
            format!("mode=relay; Trim-MC-token={}", token)
        };
        let url = format!("{}{}", base, path);

        let mut req = if is_post {
            self.http.post(&url).body(body_json)
        } else {
            self.http.get(&url)
        };
        req = req
            .header("Content-Type", "application/json")
            .header("Authorization", token)
            .header("Cookie", cookie)
            .header("Authx", authx)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) fntv-tauri2");

        let resp = req.send().await.map_err(|e| {
            if e.is_builder() {
                "服务器地址格式无效。示例：http://192.168.31.170:18888 或 100.66.1.2:18888（英文冒号；不带端口时默认 80）".to_string()
            } else if e.is_connect() {
                "无法连接服务器：请确认 NAS 地址与端口、本机与 NAS 网络互通（Tailscale 需已连接）、防火墙放行".to_string()
            } else if e.is_timeout() {
                "连接超时：网络不可达或 NAS 未响应".to_string()
            } else {
                format!("网络错误: {}", e)
            }
        })?;
        let status = resp.status().as_u16();

        if matches!(status, 301 | 302 | 307 | 308) {
            let loc = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            if !loc.is_empty() {
                let (nb, np) = split_redirect(base, &loc);
                return self.call_inner(&nb, &np, method, token, body, depth + 1).await;
            }
        }

        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();
        let text = resp.text().await.unwrap_or_default();
        if ct.contains("text/html") {
            return Err("接口返回了网页(HTML)而非 JSON——服务器地址/端口可能填错".into());
        }
        let v: Value =
            serde_json::from_str(&text).map_err(|_| format!("HTTP {} 返回非 JSON 数据", status))?;

        let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
        if code == 5000 {
            // invalid sign：随机数/时序偶发冲突，重试一次（对齐 request.ts 重试逻辑）
            return self.call_inner(base, path, method, token, body, depth).await;
        }
        if code != 0 {
            let msg = v
                .get("msg")
                .and_then(|m| m.as_str())
                .unwrap_or("未知业务错误")
                .to_string();
            return Err(msg);
        }
        Ok(v.get("data").cloned().unwrap_or(Value::Null))
        })
    }
}
