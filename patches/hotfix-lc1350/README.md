# Fntv-Plus 热补丁 lc-1350 —— PotPlayer 播放僵尸流修复（用户一键补丁包）

## 问题现象

用户用 PotPlayer 播放 NAS 上的视频时：

- PotPlayer 内显示的下载带宽只有 3~4 MB/s，时不时卡顿缓冲；
- 飞牛 NAS 一直在满速上传、电脑一直接收，千兆局域网被占满；
- **关掉 PotPlayer 之后 NAS 的上传/下载仍然不停**。

## 根因

PotPlayer 打开/扫描大文件时会连发「读几百 KB 就弃连」的探测请求
（日志实测 5 秒约 50 发）。这些请求经过应用本地 playbackShim 代理层时，
旧代码在「响应头已转发」之后就不再回收上游连接；其中带 `Range: bytes=X-`
的请求走 pipe 转发路径，pipe 之前挂的字节计数监听器会让上游保持满速流动——
**每一条被弃的请求都变成一条从该偏移一直拉到文件尾（本例 86GB）的全速僵尸流**。
几十条僵尸流把 NAS 千兆瓜分殆尽，播放流只能分到 3-4 MB/s；僵尸流挂在应用进程上，
与 PotPlayer 是否退出无关。

## 补丁原理（不改安装包）

应用自带 `patchOverlay` 文件级覆盖钩子（main.ts 第一行安装）：任何模块 require
到 `resources/app.asar` 内 `dest/...` 文件时，若 `<userData>/patches/<同相对路径>`
存在同名文件则优先加载。

本补丁从**用户自己的 app.asar** 里抽出原 `dest/main/common/playbackShim.js`，
做两处外科替换后写入 `%APPDATA%\fntv\patches\main\common\playbackShim.js`：

1. 播放器断开连接时**无条件掐断上游**（原来只在响应头尚未转发时才回收）；
2. 注入上游 `error` 守卫（主动掐断后 pres 必收到 ECONNRESET，原代码没有监听会崩主进程）。

因为只做锚点替换（锚点为长期稳定的代码段），对各个 3.8.0 构建子版本都安全；
锚点对不上时补丁直接中止、不做任何改动。每次打补丁都有回读校验，并生成
`playbackShim.js.lc1350.meta.json` 元信息（前后 SHA-256）供核查。

## 版本号显示：3.8.0-hotfix

打补丁成功后，补丁器会向 `%APPDATA%\fntv\config.json` 写入两个字段：

- `appliedPatchVersion = "3.8.0-hotfix"` —— 应用内「当前版本」的显示取值优先级
  最高（update.ts: `getAppliedPatchVersion() || ...`），重启应用后即显示 `3.8.0-hotfix`；
- `appliedPatchSignature` —— 主 exe 的 mtimeMs，与应用启动对账
  （`reconcilePatchStateOnStartup` → `computeInstallSignature`）算法完全一致。
  ⚠️ 两字段必须成对写：只写版本号不写签名，启动对账会按「未知安装状态」清空整个覆盖层。

版本串选 `3.8.0-hotfix` 是系统原生格式：更新比较（`versionGreater`）会剥
`-hotfix` 后缀按 `3.8.0` 参与纯数字比较——未来官方 3.8.1 热补丁（3.8.1 > 3.8.0）
与 3.9 全量包都能正常提示，不受影响。

应用运行中时 cmd 会先提示退出再继续（应用退出时的配置回写会覆盖标记字段）；
若标记被覆盖丢失，重新跑一遍补丁即可（幂等）。

## 使用方法

任选其一：

1. **双击运行**（自动定位安装目录：脚本所在目录 → 注册表 → 默认安装路径）；
2. 命令行指定安装目录：`Fntv-Plus热补丁-lc1350-PotPlayer僵尸流修复.cmd "D:\安装目录"`；
3. 把 cmd 文件复制到安装根目录（`resources` 的上一级）后运行。

打完后**重启飞牛影视**生效。运行中检测到应用会给出提示。

## 卸载

```
Fntv-Plus热补丁-lc1350-PotPlayer僵尸流修复.cmd restore
```

即删除覆盖文件，重启应用回到安装包原样。应用内「回滚补丁」按钮同样可以清掉它。

## 与官方热补丁的关系

- 若之后在应用内应用了官方热补丁且其包含 `main/common/playbackShim.js`，
  官方版本会覆盖本补丁（官方修复优先，语义正确）；
- 官方补丁的覆盖层清理逻辑也可能移除本补丁文件——都属正常，
  之后的正式版本已内置此修复（见 lc-1350 提交）。

## 验证情况（selftest.cjs，41 项全过）

- 真实安装版 asar：打补丁成功，覆盖内容 = 原文件 + 恰好 2 处替换；
- 合成 asar 矩阵：打补丁 / 幂等重跑 / 已修复识别 / 不兼容安全中止（exit 2，零写入）/ restore；
- 版本号标记：apply 写入 `3.8.0-hotfix` + 签名、restore 清除、「其它补丁」场景保护（不动别人的标记与文件）；
- 单文件 cmd：内嵌载荷 base64 回环一致、一键打补丁、restore、自定位模式、注册表探测路径；
- **完整生产路径**（T8）：注册表找到真实安装 → 用真实 Fntv-Plus.exe 以 ELECTRON_RUN_AS_NODE
  运行补丁器 → Electron 的 asar fs 拦截下读包（裸 archive 路径被拦截，须走
  `<archive>/<包内路径>`，补丁器已按运行时自动分策略）→ 真实签名 → restore 完整回滚。

配套源码级双向探针（`dest/_verify/zombie_stream_probe.cjs`）：旧转发逻辑在客户端断开后
3 秒继续拉满 1GB（僵尸流确认），补丁后仅 0.7MB 在途字节即停。

## 文件清单

| 文件 | 说明 |
| --- | --- |
| `../Fntv-Plus热补丁-lc1350-PotPlayer僵尸流修复.cmd` | 用户分发的单文件补丁（内嵌 patcher.js） |
| `patcher.js` | 补丁器（cmd 内嵌载荷的源码） |
| `selftest.cjs` | 端到端自测（28 项） |
| `../../scripts/make-hotpatch-cmd.mjs` | 由 patcher.js 重新生成单文件 cmd 的构建脚本 |
