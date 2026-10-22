# Linux / macOS 播放依赖说明

AniBox 在 Windows 上随包内置 mpv（`third_party/fntv-mpv/mpv.exe`）。
在 **Linux / macOS** 上，为控制安装包体积与授权成本，**不内置 mpv 二进制**，
而是使用系统安装的 mpv —— 这与参考项目 Fntv-Plus 的做法一致。

## 1. 安装 mpv

### macOS

```bash
brew install mpv
```

支持的查找位置（按优先级）：

1. `/opt/homebrew/bin/mpv`（Apple Silicon）
2. `/usr/local/bin/mpv`（Intel / 手动安装）
3. `/Applications/mpv.app/Contents/MacOS/mpv`
4. `PATH` 中的 `mpv`

### Linux

```bash
# Debian / Ubuntu
sudo apt install mpv

# Fedora
sudo dnf install mpv

# Arch
sudo pacman -S mpv
```

支持的查找位置（按优先级）：

1. `/usr/bin/mpv`
2. `/usr/local/bin/mpv`
3. `/snap/bin/mpv`
4. `/usr/games/mpv`
5. `/opt/mpv/bin/mpv`
6. `PATH` 中的 `mpv`

若以上都找不到，应用会在播放时给出明确提示，并可在「设置 → 播放器」里手动指定 mpv 路径。

## 2. 便携配置（弹幕脚本 / mpv.conf）

首次启动时，应用会把随包的 `portable_config` 复制到用户 mpv 配置目录：

```
~/.config/mpv/
```

这样系统 mpv 会自动加载 `mpv.conf`、`input.conf`、`uosc_danmaku` 等脚本。
复制采用「**只补缺失文件、不覆盖已有文件**」策略，不会冲掉用户自己的自定义配置；
缓存、日志、播放记录、B 站 Cookie 等运行时状态不会被复制。

## 3. Go 代理

播放所需的本地 Go 代理（`127.0.0.1:22346`）在各平台由 `proxy/` 源码构建：

```bash
# Linux x64
cd proxy && CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -trimpath -o ../third_party/proxy/proxy .

# macOS Apple Silicon
cd proxy && CGO_ENABLED=0 GOOS=darwin GOARCH=arm64 go build -trimpath -o ../third_party/proxy/proxy .

# macOS Intel
cd proxy && CGO_ENABLED=0 GOOS=darwin GOARCH=amd64 go build -trimpath -o ../third_party/proxy/proxy .
```

发布流水线（`.github/workflows/build-release.yml`）会自动完成上述构建。

## 4. PotPlayer

PotPlayer 只有 Windows 版本。非 Windows 平台会忽略 PotPlayer 选项，统一使用 mpv。
