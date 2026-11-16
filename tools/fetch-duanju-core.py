# [lc-2078] 短剧核心（guoapp「红果鉴」duanju_core）拉取脚本。
# 用法：python tools/fetch-duanju-core.py [--version app-v2.0.0-112]
# 下载 guoapp 官方 Windows 便携 ZIP，解出 duanju_core.dll 放到 third_party/duanju/。
# [lc-2180] guoapp v2 架构：站源 = JS 模块（核心从数据目录 modules/ 加载）。本脚本同时
# 从官方在线模块仓库（guoapp-modules/index）拉全部站源模块（manifest.json + main.js，
# 按 index 的 sha256 逐文件校验）放到 third_party/duanju/modules/<id>/。
# third_party/ 不入库，新环境 / CI 用本脚本补齐。
# 平台口径：Windows amd64；Linux/macOS 上游分别发 libduanju_core.so / libDuanjuCore.dylib
# （apk/dmg 内），本脚本暂只处理 Windows —— 其余平台 NOT IMPLEMENTED。
import argparse
import hashlib
import json
import os
import re
import sys
import urllib.request
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT_DIR = os.path.join(ROOT, "third_party", "duanju")
MODULE_INDEX_URLS = [
    "https://raw.githubusercontent.com/guoapp-modules/index/main/index.json",
    "https://cdn.jsdelivr.net/gh/guoapp-modules/index@main/index.json",
]
MODULE_FILE_URLS = [
    "https://raw.githubusercontent.com/{repo}/{tag}/{name}",
    "https://cdn.jsdelivr.net/gh/{repo}@{tag}/{name}",
]


def http_get(url: str, timeout: int = 30) -> bytes:
    req = urllib.request.Request(url, headers={"Accept": "*/*"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.read()


def latest_release():
    req = urllib.request.Request(
        "https://api.github.com/repos/Xanerix/guoapp/releases/latest",
        headers={"Accept": "application/vnd.github+json"},
    )
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r)


def fetch_core_dll(rel: dict) -> None:
    # [lc-2180] v2.0.0 起安装包改名 guoapp-*（红果鉴/全站源合并为「小果影视」）；
    # 旧版 zhenguojian-* 兼容保留。
    asset = next(
        (
            a
            for a in rel.get("assets", [])
            if re.match(r"(?:guoapp|zhenguojian)-.*-windows-x64\.zip$", a["name"])
        ),
        None,
    )
    if not asset:
        print("[fetch-duanju] 未找到 guoapp/zhenguojian windows-x64 zip 资产", file=sys.stderr)
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


def fetch_module_file(repo: str, tag: str, name: str) -> bytes:
    last: Exception | None = None
    for tpl in MODULE_FILE_URLS:
        try:
            return http_get(tpl.format(repo=repo, tag=tag, name=name))
        except Exception as e:  # noqa: BLE001 —— 逐镜像回退，最后一个失败才抛
            last = e
    raise RuntimeError(f"模块文件拉取失败 {repo}@{tag}/{name}: {last}")


def fetch_modules() -> None:
    """[lc-2180] 从 guoapp-modules 在线索引拉全部站源模块（sha256 校验）。"""
    index = None
    for url in MODULE_INDEX_URLS:
        try:
            index = json.loads(http_get(url))
            break
        except Exception:
            continue
    if not index or not isinstance(index.get("modules"), list):
        print("[fetch-duanju] 模块索引拉取失败", file=sys.stderr)
        sys.exit(1)

    for m in index["modules"]:
        mid, repo, tag = m.get("id"), m.get("repo"), m.get("tag")
        hashes = m.get("sha256") or {}
        if not (mid and repo and tag) or not hashes:
            print(f"[fetch-duanju] 跳过无效条目 {mid}")
            continue
        mdir = os.path.join(OUT_DIR, "modules", mid)
        os.makedirs(mdir, exist_ok=True)
        ok = True
        for name in ("manifest.json", "main.js"):
            expect = hashes.get(name, "")
            data = fetch_module_file(repo, tag, name)
            actual = hashlib.sha256(data).hexdigest()
            if expect and actual != expect:
                print(f"[fetch-duanju] {mid}/{name} sha256 不符（{actual} != {expect}）", file=sys.stderr)
                ok = False
                continue
            with open(os.path.join(mdir, name), "wb") as f:
                f.write(data)
        if ok:
            print(f"[fetch-duanju] 模块 {mid} v{m.get('version')} <- {repo}@{tag}")


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

    fetch_core_dll(rel)
    fetch_modules()


if __name__ == "__main__":
    main()
