"""下载 sherpa-onnx 预编译静态库（构建期依赖）。

自动探测平台/架构并下载对应版本到 .tools/sherpa-onnx-archives/，
供 cargo 构建（SHERPA_ONNX_ARCHIVE_DIR）使用。

用法: python scripts/download_sherpa.py [--version 1.13.5]
"""
import argparse
import os
import sys
import urllib.request
from pathlib import Path

VERSION = "1.13.5"
BASE = "https://github.com/k2-fsa/sherpa-onnx/releases/download"
# 默认直连；需要代理时用 --proxy 或环境变量 https_proxy / HTTPS_PROXY。
PROXY = os.environ.get("https_proxy") or os.environ.get("HTTPS_PROXY") or ""

# 平台 → 归档文件名模板（与 sherpa-onnx-sys build.rs 一致）
ARCHIVES = {
    ("win32", "x86_64"): "sherpa-onnx-v{V}-win-x64-static-MT-Release-lib.tar.bz2",
    ("win32", "aarch64"): "sherpa-onnx-v{V}-win-arm64-static-MT-Release-lib.tar.bz2",
    ("darwin", "x86_64"): "sherpa-onnx-v{V}-osx-x64-static-lib.tar.bz2",
    ("darwin", "aarch64"): "sherpa-onnx-v{V}-osx-arm64-static-lib.tar.bz2",
    ("linux", "x86_64"): "sherpa-onnx-v{V}-linux-x64-static-lib.tar.bz2",
    ("linux", "aarch64"): "sherpa-onnx-v{V}-linux-aarch64-static-lib.tar.bz2",
}


def detect_platform_arch() -> tuple[str, str]:
    """返回 (sys.platform, arch)。"""
    import platform as _p
    if sys.platform == "win32":
        return ("win32", "x86_64")
    machine = _p.machine().lower()
    arch = "aarch64" if machine in ("aarch64", "arm64") else "x86_64"
    return (sys.platform, arch)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--version", default=VERSION, help="sherpa-onnx crate 版本")
    ap.add_argument("--proxy", default=PROXY, help="代理地址（空 = 直连）")
    ap.add_argument("--out", default=None, help="输出目录（默认 .tools/sherpa-onnx-archives）")
    args = ap.parse_args()

    plat, arch = detect_platform_arch()
    key = (plat, arch)
    if key not in ARCHIVES:
        print(f"不支持的平台/架构: {plat}/{arch}（支持: {list(ARCHIVES)}）")
        return 1
    name = ARCHIVES[key].format(V=args.version)
    url = f"{BASE}/v{args.version}/{name}"

    out_dir = Path(args.out) if args.out else Path(__file__).resolve().parent.parent / ".tools" / "sherpa-onnx-archives"
    out_dir.mkdir(parents=True, exist_ok=True)
    out = out_dir / name
    if out.exists() and out.stat().st_size > 0:
        print(f"已存在: {out}")
        return 0

    print(f"下载 sherpa-onnx 静态库: {url}\n  -> {out}")
    handlers = []
    if args.proxy:
        handlers.append(urllib.request.ProxyHandler({"http": args.proxy, "https": args.proxy}))
    opener = urllib.request.build_opener(*handlers)
    try:
        with opener.open(url, timeout=900) as r, open(out, "wb") as f:
            while True:
                chunk = r.read(262144)
                if not chunk:
                    break
                f.write(chunk)
    except Exception as e:  # noqa: BLE001
        out.unlink(missing_ok=True)
        print(f"下载失败: {e}")
        return 1
    print(f"完成: {out}（{out.stat().st_size / 1e6:.1f} MB）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
