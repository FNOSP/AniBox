# [lc-2078] 短剧核心（guoapp「红果鉴」duanju_core）拉取脚本。
# 用法：python tools/fetch-duanju-core.py [--version app-v1.0.6-106]
# 下载 guoapp 官方 Windows 便携 ZIP（zhenguojian 全站源版），解出 duanju_core.dll
# 放到 third_party/duanju/。third_party/ 不入库，新环境 / CI 用本脚本补齐。
# 平台口径：Windows amd64；Linux/macOS 上游分别发 libduanju_core.so / libDuanjuCore.dylib
# （apk/dmg 内），本脚本暂只处理 Windows —— 其余平台 NOT IMPLEMENTED。
import argparse
import json
import os
import re
import sys
import urllib.request
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT_DIR = os.path.join(ROOT, "third_party", "duanju")


def latest_release():
    req = urllib.request.Request(
        "https://api.github.com/repos/Xanerix/guoapp/releases/latest",
        headers={"Accept": "application/vnd.github+json"},
    )
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", default=None, help="guoapp release tag（默认取 latest）")
    opts = ap.parse_args()

    if opts.version:
        req = urllib.request.Request(
            f"https://api.github.com/repos/Xanerix/guoapp/releases/tags/{opts.version}",
            headers={"Accept": "application/vnd.github+json"},
        )
        with urllib.request.urlopen(req, timeout=30) as r:
            rel = json.load(r)
    else:
        rel = latest_release()

    asset = next(
        (a for a in rel.get("assets", []) if re.match(r"zhenguojian-.*-windows-x64\.zip$", a["name"])),
        None,
    )
    if not asset:
        print("[fetch-duanju] 未找到 zhenguojian windows-x64 zip 资产", file=sys.stderr)
        sys.exit(1)

    print(f"[fetch-duanju] {rel['tag_name']} <- {asset['name']}")
    zip_path = os.path.join(OUT_DIR, "_duanju.zip")
    os.makedirs(OUT_DIR, exist_ok=True)
    urllib.request.urlretrieve(asset["browser_download_url"], zip_path)
    with zipfile.ZipFile(zip_path) as z:
        data = z.read("duanju_core.dll")
    os.remove(zip_path)
    out = os.path.join(OUT_DIR, "duanju_core.dll")
    with open(out, "wb") as f:
        f.write(data)
    print(f"[fetch-duanju] 已写入 {out}（{len(data)} 字节）")


if __name__ == "__main__":
    main()
