#!/usr/bin/env python3
"""汇总 TalkSage 日志中的声纹判定质量。"""

from __future__ import annotations

import argparse
import glob
import os
import re
from collections import Counter, defaultdict
from pathlib import Path

PATTERN = re.compile(
    r"说话人判定=\[(?P<label>[^]]+)] 声纹=Some\(\((?P<decision>\w+), "
    r"(?:(?:Some\((?P<similarity>-?[0-9.]+)\))|None)\)\)"
)


def main() -> int:
    parser = argparse.ArgumentParser(description="汇总声纹判定原因与相似度")
    parser.add_argument(
        "paths",
        nargs="*",
        help="日志路径或 glob；默认 <TALKSAGE_DATA_DIR|~/.talksage>/logs/talksage.*.log",
    )
    args = parser.parse_args()
    if args.paths:
        patterns = args.paths
    else:
        data_dir = os.environ.get("TALKSAGE_DATA_DIR") or os.path.join(
            os.path.expanduser("~"), ".talksage"
        )
        patterns = [os.path.join(data_dir, "logs", "talksage.*.log")]
    files = sorted({Path(p) for pattern in patterns for p in glob.glob(pattern)})
    decisions: Counter[str] = Counter()
    labels: Counter[str] = Counter()
    similarities: dict[str, list[float]] = defaultdict(list)
    for path in files:
        for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
            match = PATTERN.search(line)
            if not match:
                continue
            decision = match.group("decision")
            decisions[decision] += 1
            labels[match.group("label")] += 1
            if match.group("similarity") is not None:
                similarities[decision].append(float(match.group("similarity")))

    total = sum(decisions.values())
    print(f"声纹报告: files={len(files)} decisions={total}")
    if total == 0:
        print("没有找到新版声纹诊断记录；请启用功能并完成一次监听。")
        return 0
    for decision, count in decisions.most_common():
        values = similarities[decision]
        stats = ""
        if values:
            stats = f" sim(avg/min/max)={sum(values)/len(values):.3f}/{min(values):.3f}/{max(values):.3f}"
        print(f"  {decision:22s} {count:4d} ({count / total:6.1%}){stats}")
    print("标签: " + ", ".join(f"{label}={count}" for label, count in labels.most_common()))
    low = decisions["LowQualityFallback"]
    if low / total > 0.30:
        print("警告: 低质量回退超过 30%，建议重新注册声纹并检查麦克风音量/环境噪声。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
