lc-1383 出包瘦身：从 portable_config/fonts/ 移出的字体（本目录被打包过滤器排除，不随安装包分发）

移出文件：NotoSerifCJK-Bold.ttc（27,290,960 字节，SIL OFL 1.1，Google Noto）
  · 内含 5 个面：Noto Serif CJK JP / KR / SC / TC / HK（Bold），每面 44,746 码位
  · 移出原因：本仓库配置与脚本零引用（弹幕显式用 Microsoft YaHei、uosc 用 Noto Sans Mono CJK SC、
    徽章/OSD 用 Segoe UI）；仅当外挂字幕显式指名 "Noto Serif CJK *" 时才会被 libass/fontconfig 命中。
    移出后该类字幕回退系统 serif（简体环境下本机已有 Noto Serif SC VF，观感基本不变）。
  · 收益：仓库分发树 −26.0MB，安装包约 −15MB（fonts 压缩比仅 1.7×）。

还原方法（任选其一）：
  1) 把本目录的 NotoSerifCJK-Bold.ttc 移回 ../fonts/
  2) 从上游整合包提取：7z e D:\GitHub\mpv-Yaozhi-package.7z "**/NotoSerifCJK-Bold.ttc" -o<目标目录>
