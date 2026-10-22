# macOS 签名与公证（GitHub Actions）

AniBox 的 macOS 安装包（`.dmg` / `.app`）在 CI 中通过 **Developer ID Application** 证书签名，
并用 **Apple notarytool** 公证后 staple。签名由 Tauri v2 的 bundler 驱动，凭据全部来自
GitHub Actions Secrets。

> 参考实现：`D:\GitHub\Fntv-Plus`（Electron 版）。其 `package.json` 的 `build.mac` 开启了
> `hardenedRuntime: true` 并引用 `build/entitlements.mac.plist`，但 CI 里用
> `CSC_IDENTITY_AUTO_DISCOVERY: false` 显式关掉了签名，实际只在本地用自签名证书测试
> （`scripts/setup-macos-signing.sh`）。因此 AniBox 采用的是同一套「hardened runtime +
> entitlements + Developer ID」思路，但补上了真正的签名与公证链路。

---

## 1. 需要配置的 Secrets

在仓库 **Settings → Secrets and variables → Actions** 中新增：

| Secret | 必填 | 说明 |
| --- | --- | --- |
| `APPLE_CERTIFICATE` | 是 | Developer ID Application 证书（`.p12`）的 **base64** 文本 |
| `APPLE_CERTIFICATE_PASSWORD` | 是 | 导出 `.p12` 时设置的密码 |
| `APPLE_SIGNING_IDENTITY` | 是 | 完整签名身份，如 `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID` | 是 | Apple Developer 账号邮箱 |
| `APPLE_APP_SPECIFIC_PASSWORD` | 是 | 该 Apple ID 的 **App 专用密码**（非登录密码） |
| `APPLE_TEAM_ID` | 是 | 10 位 Team ID（Apple Developer 后台 Membership 页可见） |

> 只要 `APPLE_CERTIFICATE` / `APPLE_ID` / `APPLE_TEAM_ID` 任一为空，工作流会**跳过签名与公证**，
> 构建继续完成（产物未签名），日志里给出 `::warning::`，作业不会失败。

---

## 2. 生成证书（.p12 → base64）

1. 在 **Apple Developer → Certificates, Identifiers & Profiles** 创建
   **Developer ID Application** 证书，下载 `.cer` 并导入钥匙串。
2. 在「钥匙串访问」中展开该证书 → 导出为 `.p12`（设置一个密码，即 `APPLE_CERTIFICATE_PASSWORD`）。
3. 转成 base64（macOS）：

   ```bash
   base64 -i DeveloperID.p12 | pbcopy
   ```

   Linux / Windows(Git Bash)：

   ```bash
   base64 -w0 DeveloperID.p12
   ```

4. 把输出粘贴到 Secret `APPLE_CERTIFICATE`。

确认签名身份字符串：

```bash
security find-identity -v -p codesigning
# 复制 "Developer ID Application: ... (XXXXXXXXXX)" 整行到 APPLE_SIGNING_IDENTITY
```

---

## 3. 生成 App 专用密码

1. 打开 <https://appleid.apple.com> → 登录。
2. **登录与安全 → App 专用密码 → 生成**。
3. 名称随意（如 `AniBox CI`），复制生成的 `xxxx-xxxx-xxxx-xxxx`。
4. 粘贴到 Secret `APPLE_APP_SPECIFIC_PASSWORD`。

> 若开启了双重认证，必须使用 App 专用密码，不能使用账号密码。

---

## 4. 工作流做了什么

`.github/workflows/build-release.yml` 中新增的 macOS 步骤（仅 `runner.os == 'macOS'`）：

1. **检测 macOS 签名凭据**：判断 Secrets 是否齐全，输出 `enabled`。
2. **导入 macOS 签名证书到临时 keychain**（仅启用时）：
   - base64 解码 p12 到 `$RUNNER_TEMP`（用完即删）
   - `security create-keychain` / `unlock-keychain` / `import`
   - `security set-key-partition-list` 允许 `codesign` 无交互访问私钥
   - `security list-keychains -s` 把临时 keychain 加入搜索列表
   - 通过 `$GITHUB_ENV` 注入 `APPLE_SIGNING_IDENTITY` / `APPLE_ID` / `APPLE_PASSWORD` / `APPLE_TEAM_ID`
     （**不传** `APPLE_CERTIFICATE` 给 tauri，避免二次导入造成签名身份歧义）
3. **构建**：`npx tauri build --bundles dmg`，Tauri 自动用 notarytool 公证并 staple。
4. **校验**：对 `.app` 执行 `codesign -dv --verbose=4`、`codesign --verify --deep --strict`、
   `spctl -a -vv`，结果打印到日志（不含敏感信息）。
5. **清理临时 keychain**（`always()`）。

## 5. entitlements

`src-tauri/entitlements.plist`，由 `src-tauri/tauri.macos.conf.json` 的
`bundle.macOS.entitlements` 引用，`hardenedRuntime: true`。内容（按 AniBox 实际需要裁剪自
Fntv-Plus）：

- `com.apple.security.cs.allow-jit` / `allow-unsigned-executable-memory`（WKWebView）
- `com.apple.security.cs.disable-library-validation` / `allow-dyld-environment-variables`（外部 mpv / sidecar）
- `com.apple.security.network.client` / `network.server`（fnOS 访问 + 本地代理/danmu-api）
- `com.apple.security.files.user-selected.read-write` / `downloads.read-write`（本地文件与缓存）

## 6. 本地验证（可选）

```bash
codesign -dv --verbose=4 "AniBox.app"
codesign --verify --deep --strict --verbose=2 "AniBox.app"
spctl -a -vv -t exec "AniBox.app"
xcrun stapler validate "AniBox.app"
```
