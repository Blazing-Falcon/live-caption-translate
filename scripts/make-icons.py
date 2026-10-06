"""Generates the placeholder app icon (a caption bar glyph) with the standard library only."""
import struct, zlib, sys
from pathlib import Path

def png(size):
    rows = []
    for y in range(size):
        row = bytearray([0])
        for x in range(size):
            u, v = x / size, y / size
            inside = 0.04 <= u <= 0.96 and 0.04 <= v <= 0.96
            r = 0.16
            cx, cy = min(max(u, 0.04 + r), 0.96 - r), min(max(v, 0.04 + r), 0.96 - r)
            inside = inside and (u - cx) ** 2 + (v - cy) ** 2 <= r * r
            bar1 = 0.2 <= u <= 0.8 and 0.55 <= v <= 0.65
            bar2 = 0.3 <= u <= 0.7 and 0.72 <= v <= 0.8
            if not inside:
                row += bytes([0, 0, 0, 0])
            elif bar1 or bar2:
                row += bytes([0xE3, 0xA3, 0x3B, 255])
            else:
                row += bytes([0x0E, 0x0F, 0x11, 255])
        rows.append(bytes(row))
    raw = zlib.compress(b"".join(rows), 9)
    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", raw) + chunk(b"IEND", b""))

out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)
sizes = [16, 32, 48, 256]
images = [png(s) for s in sizes]
(out / "icon.png").write_bytes(png(256))
header = struct.pack("<HHH", 0, 1, len(sizes))
offset = 6 + 16 * len(sizes)
entries = b""
for s, data in zip(sizes, images):
    entries += struct.pack("<BBBBHHII", 0 if s >= 256 else s, 0 if s >= 256 else s, 0, 0, 1, 32, len(data), offset)
    offset += len(data)
(out / "icon.ico").write_bytes(header + entries + b"".join(images))
print("icons written to", out)
