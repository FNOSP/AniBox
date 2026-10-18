# tests/ —— 测试 / 验证 / 一次性脚本与产物

按项目 `AGENTS.md` 约定：**测试、验证、一次性探针脚本及临时产物统一放这里，不要放到项目根目录。**

目录约定：

| 目录 | 用途 |
| --- | --- |
| `tests/` | 验证 / 一次性脚本（如 `verify-*.mjs`、`tmp-*.mjs`） |
| `tests/logs/` | 运行日志（`*.log`、`.devlog*.txt`） |
| `tests/artifacts/` | 截图、临时文件等产物 |
| `tests/spikes/` | 一次性技术验证（spike）工程 |

> 这些内容默认不入库（见根目录 `.gitignore`），仅作本地开发佐证。
