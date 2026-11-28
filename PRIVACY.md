# AniBox 隐私说明

> 最后更新：2026-11-28

AniBox 是本地优先的影视增强应用：你的个人数据（设置、凭证、缓存、播放记录）始终保存在**你自己的设备与 NAS** 上，不经手任何 AniBox 开发者的服务器。本文说明各功能的数据流向。

## Trakt 同步（设备授权）

- 连接 Trakt 采用 OAuth **设备授权流程**：AniBox 向 Trakt 官方接口申请授权码，你在浏览器登录 Trakt 并确认后，AniBox 获得访问令牌（access token / refresh token）。
- 令牌**仅保存在你自己的 NAS 本地设置中**，用于调用 Trakt 官方 API 写入观影历史与播放进度（scrobble）。**不经过、也不存储于任何 AniBox 开发者的服务器。**
- 在 AniBox 设置中「断开连接」会立即调用 Trakt 官方接口**吊销该令牌**；你也可以随时在 Trakt 账号设置中撤销授权。
- AniBox 使用 Trakt 但不受 Trakt 认可或认证（not endorsed or certified by Trakt）。

## 影视元数据与图片

- 元数据请求（TMDB / Bangumi / AniList / 豆瓣等）只包含**公开的作品标识**（如标题、TMDB ID），不包含你的个人信息或你正在观看的具体文件路径。
- 部分公开元数据经 AniBox 社区维护的镜像服务加速；镜像仅代理公开数据请求，不记录账号信息。

## 弹幕

- 弹幕来自弹弹play 开放平台、B站及自建弹幕源，按**作品标题与集数**匹配。
- 弹幕匹配结果与缓存保存在你的 NAS 本地。
- 使用弹弹play 官方开放 API 时遵循其开放 API 条款与署名要求。

## 匿名使用统计（可选，默认可关闭）

- 设置面板「关于」页可开关的匿名心跳：每天最多一次，仅上报**随机匿名 ID、版本号、系统类型、日期**。
- **不含**账号、IP、媒体库名称、文件路径或任何观看内容；断网静默失败并在恢复后补报；关闭或未配置服务端一律不发。

## 凭据的存储

- 所有登录凭据（Trakt 令牌、Bangumi Token、B站 Cookie、弹弹play 凭证、飞牛账号）只保存在你自己的设备或 NAS 本地，不随包分发、不上传。

## 联系方式

- 问题与数据相关请求请通过 GitHub Issues：https://github.com/FNOSP/AniBox/issues
