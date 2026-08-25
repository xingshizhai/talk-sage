#!/usr/bin/env python3
"""
下载 AISHELL-1 测试集（仅测试分区 + transcript）用于 bench_cer 基准测试。

用法：
  python scripts/download_aishell1_test.py [--out-dir ./bench_data/aishell1]

下载后目录结构：
  bench_data/aishell1/
    wav/test/<speaker_id>/*.wav   # 7176 条 wav，每条约 3-5 秒，16kHz mono
    transcript.txt                 # bench_cer 格式：<wav_id> <参考文本>

注意：AISHELL-1 完整包约 1.5GB，脚本只会解压测试分区（约 200MB），
      原始 tgz 下载后可以删掉。
"""

import argparse
import os
import sys
import tarfile
import urllib.request
from pathlib import Path

# OpenSLR 33 —— AISHELL-1 官方源
PACKAGE_URL = "https://openslr.magicdatatech.com/resources/33/data_aishell.tgz"
TRANSCRIPT_URL = "https://openslr.magicdatatech.com/resources/33/resource_aishell.tgz"

def download(url: str, dest: Path, desc: str) -> None:
    if dest.exists():
        print(f"已存在，跳过下载: {dest}")
        return
    dest.parent.mkdir(parents=True, exist_ok=True)
    print(f"下载 {desc}...")
    print(f"  URL: {url}")
    print(f"  目标: {dest}")

    def progress(block_num, block_size, total_size):
        if total_size > 0:
            pct = min(block_num * block_size / total_size * 100, 100)
            mb = block_num * block_size / 1e6
            print(f"\r  {pct:.1f}%  {mb:.0f} MB", end="", flush=True)

    urllib.request.urlretrieve(url, dest, progress)
    print()


def extract_test_wavs(tgz_path: Path, out_dir: Path) -> None:
    wav_out = out_dir / "wav" / "test"
    if wav_out.exists() and any(wav_out.rglob("*.wav")):
        print(f"测试 wav 已存在: {wav_out}")
        return
    wav_out.mkdir(parents=True, exist_ok=True)
    print(f"解压测试 wav → {wav_out} ...")
    count = 0
    with tarfile.open(tgz_path, "r:gz") as tar:
        for member in tar.getmembers():
            # 只要 wav/test/ 下的文件
            if "/wav/test/" in member.name and member.name.endswith(".wav"):
                # 压平到 wav/test/<speaker>/<wav_id>.wav
                parts = member.name.split("/wav/test/", 1)
                rel = parts[1]  # e.g. S0003/BAC009S0003W0001.wav
                dest = wav_out / rel
                dest.parent.mkdir(parents=True, exist_ok=True)
                with tar.extractfile(member) as f_in, open(dest, "wb") as f_out:
                    f_out.write(f_in.read())
                count += 1
                if count % 500 == 0:
                    print(f"  已解压 {count} 条...")
    print(f"解压完成：{count} 条 wav")


def build_transcript(resource_tgz: Path, out_dir: Path) -> None:
    tx_out = out_dir / "transcript.txt"
    if tx_out.exists():
        print(f"transcript 已存在: {tx_out}")
        return

    print("提取 transcript...")
    # resource_aishell.tgz 里有 resource_aishell/lexicon/aishell_transcript_v0.8.txt
    raw_lines: list[str] = []
    with tarfile.open(resource_tgz, "r:gz") as tar:
        for member in tar.getmembers():
            if "transcript" in member.name and member.name.endswith(".txt"):
                with tar.extractfile(member) as f:
                    raw_lines = f.read().decode("utf-8").splitlines()
                print(f"  读取 {member.name}，{len(raw_lines)} 行")
                break

    if not raw_lines:
        print("错误：找不到 transcript 文件", file=sys.stderr)
        sys.exit(1)

    # 找测试集 wav id 集合（用已解压的文件）
    wav_dir = out_dir / "wav" / "test"
    test_ids = {p.stem for p in wav_dir.rglob("*.wav")}
    print(f"  测试集 wav id 数量：{len(test_ids)}")

    # 过滤出测试集行，格式保持 "<id> <text>"
    out_lines: list[str] = []
    for line in raw_lines:
        line = line.strip()
        if not line:
            continue
        wav_id = line.split()[0]
        if wav_id in test_ids:
            out_lines.append(line)

    tx_out.write_text("\n".join(out_lines), encoding="utf-8")
    print(f"  写出 {len(out_lines)} 条测试 transcript → {tx_out}")


def main():
    parser = argparse.ArgumentParser(description="下载 AISHELL-1 测试集")
    parser.add_argument("--out-dir", default="./bench_data/aishell1",
                        help="输出目录（默认 ./bench_data/aishell1）")
    args = parser.parse_args()

    out_dir = Path(args.out_dir).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    tmp = out_dir / "_tmp"
    tmp.mkdir(exist_ok=True)

    data_tgz = tmp / "data_aishell.tgz"
    resource_tgz = tmp / "resource_aishell.tgz"

    download(PACKAGE_URL, data_tgz, "AISHELL-1 音频包（~1.5GB）")
    download(TRANSCRIPT_URL, resource_tgz, "AISHELL-1 资源包（transcript）")

    extract_test_wavs(data_tgz, out_dir)
    build_transcript(resource_tgz, out_dir)

    wav_count = len(list((out_dir / "wav" / "test").rglob("*.wav")))
    tx_count = sum(1 for _ in open(out_dir / "transcript.txt"))
    print(f"""
准备完成！
  wav 数量  : {wav_count}
  transcript: {tx_count} 条
  位置      : {out_dir}

运行基准测试：
  cargo run -p talksage-asr --bin bench_cer --release -- \\
    <models-root> \\
    {out_dir}/wav/test \\
    {out_dir}/transcript.txt \\
    --max 500   # 先跑 500 条快速验证

完整测试（7176 条）去掉 --max 即可。
""")


if __name__ == "__main__":
    main()
