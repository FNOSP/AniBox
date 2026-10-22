# 安全说明：飞牛 fnOS 影视客户端签名材料（Authx）

## 这对 key 是什么

`src-tauri/src/authx.rs`（以及弹幕 shim `assets/danmaku-shim.cjs`）用一对
`API_KEY` / `API_SECRET` 给飞牛 fnOS 网关请求计算 `Authx` 签名头：

```
sign = md5(API_KEY + "_" + path + "_" + nonce + "_" + timestamp + "_" + md5(body) + "_" + API_SECRET)
Authx: nonce=...&timestamp=...&sign=...
```

这对值是**飞牛官方 Web 影视客户端内置的公开客户端凭据**（随官方前端 JS 一起下发，
任何浏览器打开官方 Web 端都能拿到），并非用户账号密码或私有服务端密钥。
它只用于证明「请求来自官方客户端形态」，不授予额外权限——真正的身份仍由
`Authorization` token 与网关 Cookie 决定。

## 为什么改成构建期注入

- 硬编码在源码里会随仓库公开，观感等同于「内嵌官方签名密钥」，且一旦官方轮换就需要改源码重发。
- 改为从本地文件/环境变量注入后：源码与产物不含明文；轮换时只需改本地 `secrets/fn-authx.local` 重新构建。
- 语义与仓库内 `fpk/src/go/internal/bridge/bridge.go` 的 `FNTV_AUTHX_KEY` / `FNTV_AUTHX_SECRET` 一致：
  **未注入 = 不发 `Authx` 头**，请求照常发出，由网关按未签名请求处理（不崩溃）。

## 如何配置

优先级（高 → 低）：

1. **运行时环境变量**（本地开发最方便）：
   ```
   ANIBOX_FN_AUTHX_KEY=...  ANIBOX_FN_AUTHX_SECRET=...  cargo run
   ```
2. **构建期文件** `secrets/fn-authx.local`（已被 `.gitignore` 忽略，不会入库）：
   ```
   ANIBOX_FN_AUTHX_KEY=<客户端 key>
   ANIBOX_FN_AUTHX_SECRET=<客户端 secret，UUID 形态>
   ```
   兼容简写 `KEY=` / `SECRET=`。示例见 `secrets/fn-authx.local.example`（占位值，可入库）。
   `src-tauri/build.rs` 读取后以 `cargo:rustc-env=ANIBOX_FN_AUTHX_KEY=...` 注入编译期环境，
   Rust 侧用 `option_env!` 读取；弹幕 shim 由 Rust 拉起时通过 `Command::env` 传给 Node。

## 未配置时的降级行为

- 应用正常启动、播放、B站弹幕、本地弹幕 shim 的 `/health`、`/danmaku*` 端点**均不受影响**。
- 需要飞牛网关签名的接口会**不带 `Authx` 头**发出（`fnapi.rs` 与 shim 的 `fnCall` 均如此）：
  - **NAS 字幕搜索 / 下载**（`/nas-subtitle*`）通常被网关拒绝 → 功能不可用。
  - 其它走 `fnapi.rs` 的飞牛 API 若网关强制校验签名，也可能失败。
- 构建时会打印 `cargo:warning=未注入飞牛签名材料 ...` 提示。

## 官方轮换后如何处理

1. 从最新官方 Web 客户端 JS 资源中提取新的 key/secret。
2. 更新本地 `secrets/fn-authx.local`（或运行时环境变量）。
3. 重新构建 / 打包。无需修改任何源码。
4. 若旧值曾以明文进入过任何提交/产物，按「先轮换、再清理引用、历史视为已泄露」处理。
