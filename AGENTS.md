# Fntv-Plus-Tauri2 —— 项目规则

---

# 入库规则（GitHub 只放「纯净代码」）

**一句话**：仓库里只保留「别人 clone 下来能读懂、能构建」的必需代码。密钥、部署脚手架、测试/验证脚本、本地状态与个人数据，一律不入库。

## 一、绝不入库的四大类

### 1. 密钥 / 凭据（最高优先级）
`.secret.local`、`.status-token.local`、`.workers-url.local`、`.tmdb-key.local`、`.env*`、`*.pem`、`*.key`、`.wrangler/`
- 本项目的镜像准入密钥（`FNTV_BGM_SECRET` / `FNTV_TMDB_SECRET`）就是典型：**一旦提交就等于公开**（GitHub 上的密钥会被爬虫按秒扫到）。真发生了：**先轮换密钥**，再 `git rm --cached`，最后才谈清历史 —— 历史里的密钥永远算泄露。
- 已发生过的实例：`bgm-mirror/.status-token.local` 曾没被 `.gitignore` 覆盖，而该目录整体未跟踪，一次 `git add -A` 就会带上它。补规则时务必按「目录 + 同类文件」一起盖。

### 2. 部署 / 运维脚手架与文档
`bgm-mirror/`、`tmdb-mirror/`、`stats-server/` 整个目录（含各自的 `README.md`、`deploy.mjs`、`部署.cmd`）、`RELEASE.md`、`release.cmd`
- 这些是**「我们自己的服务器怎么搭」**的内部资料。公开出去只增加被刷、被抄、被盯上的面，对使用者没有任何价值。

### 3. 测试 / 验证脚本与一次性探针
`scripts/test-*.mjs`、`scripts/verify-*.mjs`、`**/verify_*.js`、`**/verify-*.js`、`*_test.go`、`spike-*/`、`tools/` 下的临时校验脚本、`.dryrun*.cmd`
- 它们是开发期的佐证，不是产品的一部分。（哪天决定开源测试，再显式 `git add -f` 单独决定 —— 不要默认放开。）

### 4. 本地状态 / 个人数据 / 工具目录
`.rea-*/`、`.workbuddy/`、`node_modules/`、`dist/`、`target/`、`nul`、`on`、`third_party/fntv-mpv/portable_config/files/recent.json`、`persistent_config.json`
- `recent.json` 里是**你自己的播放记录**，`persistent_config.json` 是 mpv 运行时自动写的状态：两者每次运行都会被改写（表现为「工作区永远是脏的」），而且随包分发没有任何意义。

## 二、入库前自检（三步，别跳）

```bash
git status --short                                          # 1. 有没有不该出现的东西
git add -An | grep -iE 'secret|token|\.env|deploy|verify|_test|spike|\.local'   # 2. 预演：命中禁入项就停下来看清楚
git diff --cached --stat                                    # 3. 最终确认改动范围
```

- **一个改动一个 commit**。
- push 约定按各仓库现有规矩走（`Fntv-net` 目前是**只本地提交、不 push**）。
- `.gitignore` 只对**未跟踪**文件生效。发现某个禁入文件**已经**被跟踪（`git ls-files` 里有它），必须显式 `git rm --cached <file>` 把它移出索引（本地文件不受影响），否则它每次 commit 都会继续上传。
- 新增同类文件时**同时补 `.gitignore` 规则**，不要指望下一个人记得。
