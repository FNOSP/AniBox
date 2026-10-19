#!/usr/bin/env bash
# build-fpk.sh —— 为飞牛 FPK 构建 Go 后端二进制。
#
# 产物（相对 fntvplus 仓库根）：
#   app/server/fntvplus        linux/amd64 二进制（fnOS x86 包，fnpack 只打包 app/ 目录内容）
#   app/server/fntvplus-arm64  linux/arm64 二进制（可选，ARM 包用；x86 测试包可不编）
#
# 重要：fnpack build 会把仓库根的 app/ 目录打成 app.tgz 放进 .fpk，
#       安装后 app/ 内容直接落在 TRIM_APPDEST 根 —— 所以二进制必须在
#       app/server/fntvplus（对应运行时 $TRIM_APPDEST/server/fntvplus，
#       与 cmd/main 的 BIN 路径一致）。根目录 server/ 不会进包！
#
# 用法（在 AniBox 仓库的 fpk/ 目录执行；脚本按自身位置定位，路径无关）：
#   bash scripts/build-fpk.sh            # 仅 amd64
#   WITH_ARM64=1 bash scripts/build-fpk.sh  # amd64 + arm64
#
# 前置：Go 1.23+（交叉编译无需目标平台工具链，纯标准库）。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

GO="${GO:-go}"

# —— 镜像准入密钥注入（编译期 -X；缺失不失败，运行期回退直连）——
# 来源优先级：进程环境变量 → 仓库根 secrets/<mirror>/.secret.local。
# 支持 `KEY=value`（tmdb）与纯值（bgm，base64 末尾自带 `=`）两种格式；不打印密钥值。
REPO_ROOT="$(cd "$ROOT/.." && pwd)"
read_secret() { # $1=环境变量名 $2=文件路径 $3=文件内键名
  local env_name="$1" file="$2" key="$3" val=""
  if [ -n "${!env_name:-}" ]; then
    val="${!env_name}"
  elif [ -f "$file" ]; then
    IFS= read -r val < "$file" || true
    val="${val%$'\r'}"
    if [ "${val#"$key="}" != "$val" ]; then
      val="${val#"$key="}"
    fi
    # 去首尾空白与成对引号
    val="${val#"${val%%[![:space:]]*}"}"
    val="${val%"${val##*[![:space:]]}"}"
    val="${val%\"}"; val="${val#\"}"
    val="${val%\'}"; val="${val#\'}"
  fi
  case "$val" in
    *[[:space:]]*) val="" ;; # 含空白视为无效（避免 ldflags 解析问题）
  esac
  printf '%s' "$val"
}

TMDB_SECRET="$(read_secret FNTV_TMDB_SECRET "$REPO_ROOT/secrets/tmdb-mirror/.secret.local" FNTV_TMDB_SECRET)"
BGM_SECRET="$(read_secret FNTV_BGM_SECRET "$REPO_ROOT/secrets/bgm-mirror/.secret.local" FNTV_BGM_SECRET)"
LDFLAGS=""
[ -n "$TMDB_SECRET" ] && LDFLAGS="$LDFLAGS -X fntvplus/internal/mirror.TMDBSecret=$TMDB_SECRET"
[ -n "$BGM_SECRET" ] && LDFLAGS="$LDFLAGS -X fntvplus/internal/mirror.BGMSecret=$BGM_SECRET"
LDFLAGS="${LDFLAGS# }"
LD_ARGS=()
[ -n "$LDFLAGS" ] && LD_ARGS=(-ldflags "$LDFLAGS")
if [ -n "$LDFLAGS" ]; then
  echo "==> 已注入镜像准入密钥（TMDB/BGM）"
else
  echo "==> [warn] 未找到镜像密钥，运行期将回退直连"
fi

echo "==> building linux/amd64 -> app/server/fntvplus ..."
mkdir -p app/server
( cd src/go && GOOS=linux GOARCH=amd64 "$GO" build ${LD_ARGS[@]+"${LD_ARGS[@]}"} -o ../app/server/fntvplus ./cmd/fntvplus )

if [ "${WITH_ARM64:-0}" = "1" ]; then
  echo "==> building linux/arm64 -> app/server/fntvplus-arm64 ..."
  ( cd src/go && GOOS=linux GOARCH=arm64 "$GO" build ${LD_ARGS[@]+"${LD_ARGS[@]}"} -o ../app/server/fntvplus-arm64 ./cmd/fntvplus )
fi

echo "==> done. Files:"
ls -la app/server/
echo ""
echo "==> next: run fnpack build (or Windows 上直接双击 build-fpk.exe 一键打包)"
