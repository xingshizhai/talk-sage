"""TalkSage 图标生成器（纯标准库，无 PIL 依赖）。

生成:
  web/src-tauri/icons/{16,32,48,64,128,256,512}x{size}.png
  web/src-tauri/icons/icon.png (512)
  web/src-tauri/icons/icon.ico  (PNG-in-ICO, Windows)
  web/src-tauri/icons/icon.icns (ic08/ic09/ic10, macOS)

图案: 深色圆角方块 + 青色对话气泡点（极简品牌）。
"""
import os
import struct
import zlib
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "web" / "src-tauri" / "icons"

BG = (15, 23, 42, 255)      # #0f172a 深蓝黑
FG = (45, 212, 191, 255)    # #2dd4bf 青色（与旧版术语强调色一致）


def make_rgba(size: int) -> bytes:
    """生成 size×size RGBA 像素: 圆角方块 + 居中气泡点。"""
    rows = []
    radius = max(2, size // 8)
    cx, cy = size / 2, size / 2
    r_dot = size * 0.22
    # 气泡: 大圆 + 小尾巴圆
    tail_x, tail_y = cx + size * 0.16, cy + size * 0.24
    r_tail = size * 0.09
    for y in range(size):
        row = bytearray()
        for x in range(size):
            # 圆角判定
            dx = max(radius - x, x - (size - 1 - radius), 0)
            dy = max(radius - y, y - (size - 1 - radius), 0)
            if dx * dx + dy * dy > radius * radius:
                row += bytes(BG[:3] + (0,))  # 圆角外透明
                continue
            # 气泡点
            d1 = (x - cx) ** 2 + (y - cy) ** 2
            d2 = (x - tail_x) ** 2 + (y - tail_y) ** 2
            if d1 <= r_dot * r_dot or d2 <= r_tail * r_tail:
                row += bytes(FG)
            else:
                row += bytes(BG)
        rows.append(bytes(row))
    # PNG 编码
    raw = b"".join(b"\x00" + r for r in rows)  # filter 0 per scanline
    def chunk(tag: bytes, data: bytes) -> bytes:
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def write_ico(pngs: dict[int, bytes], path: Path) -> None:
    """PNG-in-ICO 容器（Windows Vista+ 支持 PNG 压缩条目）。"""
    entries = []
    offset = 6 + 16 * len(pngs)
    for size in sorted(pngs):
        data = pngs[size]
        w = 0 if size >= 256 else size
        h = 0 if size >= 256 else size
        entries.append(
            struct.pack("<BBBBHHII", w, h, 0, 0, 1, 32, len(data), offset)
        )
        offset += len(data)
    with open(path, "wb") as f:
        f.write(struct.pack("<HHH", 0, 1, len(pngs)))
        for e in entries:
            f.write(e)
        for size in sorted(pngs):
            f.write(pngs[size])


def write_icns(pngs: dict[int, bytes], path: Path) -> None:
    """ICNS 容器: ic08=128, ic09=256, ic10=512（均为 PNG 数据条目）。"""
    types = {128: b"ic08", 256: b"ic09", 512: b"ic10"}
    body = b""
    for size, t in types.items():
        if size in pngs:
            data = pngs[size]
            body += t + struct.pack(">I", len(data) + 8) + data
    with open(path, "wb") as f:
        f.write(b"icns" + struct.pack(">I", len(body) + 8) + body)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    sizes = [16, 32, 48, 64, 128, 256, 512]
    pngs = {s: make_rgba(s) for s in sizes}
    for s in sizes:
        (OUT / f"{s}x{s}.png").write_bytes(pngs[s])
    (OUT / "icon.png").write_bytes(pngs[512])
    (OUT / "32x32.png").write_bytes(pngs[32])
    (OUT / "128x128.png").write_bytes(pngs[128])
    (OUT / "128x128@2x.png").write_bytes(pngs[256])
    write_ico({16: pngs[16], 32: pngs[32], 48: pngs[48], 64: pngs[64], 128: pngs[128], 256: pngs[256]}, OUT / "icon.ico")
    write_icns({128: pngs[128], 256: pngs[256], 512: pngs[512]}, OUT / "icon.icns")
    for f in sorted(OUT.iterdir()):
        print(f"{f.name:20s} {f.stat().st_size:>8} bytes")


if __name__ == "__main__":
    main()
