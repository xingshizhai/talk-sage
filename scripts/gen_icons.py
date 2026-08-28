"""TalkSage 图标生成器（纯标准库，无 PIL 依赖）。

生成:
  web/src-tauri/icons/{16,32,48,64,128,256,512}x{size}.png
  web/src-tauri/icons/icon.png (512)
  web/src-tauri/icons/icon.ico  (PNG-in-ICO, Windows)
  web/src-tauri/icons/icon.icns (ic07/ic08/ic09/ic10, macOS)

图案: 深色圆角方块 + 蓝青双色交叠圆（与 logo/talksage-icon.svg 一致）。
"""
import os
import struct
import zlib
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "web" / "src-tauri" / "icons"

BG = (16, 35, 63, 255)       # #10233F
LEFT = (126, 166, 255, 255)  # #7EA6FF
RIGHT = (79, 216, 221, 255)  # #4FD8DD


def make_rgba(size: int, artwork_scale: float = 1.0) -> bytes:
    """生成 size×size RGBA 像素: 圆角方块 + 蓝青双色交叠圆。

    ``artwork_scale`` 用于给 macOS 图标保留系统视觉安全区。Dock 会把 ICNS
    的整个画布当作图标尺寸；如果底板铺满画布，会比系统图标显得大一圈。
    """
    rows = []
    artwork_size = size * artwork_scale
    inset = (size - artwork_size) / 2
    radius = max(2, artwork_size / 8)
    cx, cy = size / 2, size / 2
    left_x = cx - artwork_size * 0.125
    right_x = cx + artwork_size * 0.125
    circle_radius = artwork_size * 17 / 64
    for y in range(size):
        row = bytearray()
        for x in range(size):
            # 圆角判定
            dx = max(inset + radius - x, x - (size - 1 - inset - radius), 0)
            dy = max(inset + radius - y, y - (size - 1 - inset - radius), 0)
            if dx * dx + dy * dy > radius * radius:
                row += bytes(BG[:3] + (0,))  # 圆角外透明
                continue
            # 与 talksage-icon.svg 相同：两个圆的交叠区域露出深色底板。
            in_left = (x - left_x) ** 2 + (y - cy) ** 2 <= circle_radius**2
            in_right = (x - right_x) ** 2 + (y - cy) ** 2 <= circle_radius**2
            if in_left and in_right:
                row += bytes(BG)
            elif in_left:
                row += bytes(LEFT)
            elif in_right:
                row += bytes(RIGHT)
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
    """ICNS 容器（均为 PNG 数据条目）。"""
    types = {128: b"ic07", 256: b"ic08", 512: b"ic09", 1024: b"ic10"}
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
    # macOS 的图标画布包含约 10% 的四周安全区，以匹配系统图标的视觉尺寸。
    mac_pngs = {s: make_rgba(s, artwork_scale=0.8) for s in (128, 256, 512, 1024)}
    write_icns(mac_pngs, OUT / "icon.icns")
    for f in sorted(OUT.iterdir()):
        print(f"{f.name:20s} {f.stat().st_size:>8} bytes")


if __name__ == "__main__":
    main()
