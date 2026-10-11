#!/usr/bin/env python3
"""从已提交的 256px 渲染缩小 Lanwork 应用图标。

源文件是 crates/app/assets/lanwork.svg，256px 渲染是同目录的 lanwork-256.png。
两份都按用户给出的字节原样提交，本脚本不改它们，也不重新解释 SVG。
缩小只用整数面积平均，不修花瓣、不改颜色。

写出：

- lanwork-{16,20,24,32,40,48,64}.png
- lanwork-32.rgba（托盘底图，自上而下 RGBA）
- lanwork.ico（16/20/24/32/40/48/64/256，BMP，供 rc.exe 嵌入）

只用 Python 标准库。PNG 用未压缩的 zlib 存储块，字节不随 zlib 版本变化。
构建不运行本脚本。在仓库根目录执行：

    python3 tools/render_app_icon.py
"""

from __future__ import annotations

import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ASSET_DIR = ROOT / "crates" / "app" / "assets"
SVG_PATH = ASSET_DIR / "lanwork.svg"
PNG_256 = ASSET_DIR / "lanwork-256.png"
SIZES = (16, 20, 24, 32, 40, 48, 64, 256)

# 用户给出的源文件和 256px 渲染。变了就说明图形被改过，先停下来。
SVG_SHA256 = "00de75e2609f8e72b7c38a21e0f559ae36bd998d45a9df4604d0cc8554b5c1bc"
PNG_256_SHA256 = "ae8c64457572d20dac4675390f7ed42af9443a211069d211f709e38fe35a0769"


def sha256(data: bytes) -> str:
    import hashlib

    return hashlib.sha256(data).hexdigest()


def paeth(a: int, b: int, c: int) -> int:
    estimate = a + b - c
    da = abs(estimate - a)
    db = abs(estimate - b)
    dc = abs(estimate - c)
    if da <= db and da <= dc:
        return a
    if db <= dc:
        return b
    return c


def decode_png(data: bytes) -> tuple[int, int, bytes]:
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise SystemExit("lanwork-256.png 不是 PNG")
    pos = 8
    width = height = None
    idat = bytearray()
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        kind = data[pos + 4 : pos + 8]
        chunk = data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if kind == b"IHDR":
            width, height, depth, color, comp, filt, interlace = struct.unpack(">IIBBBBB", chunk)
            if (depth, color, comp, filt, interlace) != (8, 6, 0, 0, 0):
                raise SystemExit("只接受 8 位非隔行 RGBA PNG")
        elif kind == b"IDAT":
            idat += chunk
        elif kind == b"IEND":
            break
    if width != 256 or height != 256:
        raise SystemExit(f"256px 渲染的尺寸是 {width}x{height}")
    raw = zlib.decompress(bytes(idat))
    stride = width * 4
    rows = []
    src = 0
    for y in range(height):
        filter_type = raw[src]
        src += 1
        row = bytearray(raw[src : src + stride])
        src += stride
        prev = rows[-1] if y else bytearray(stride)
        if filter_type == 0:
            pass
        elif filter_type == 1:
            for i in range(stride):
                left = row[i - 4] if i >= 4 else 0
                row[i] = (row[i] + left) & 0xFF
        elif filter_type == 2:
            for i in range(stride):
                row[i] = (row[i] + prev[i]) & 0xFF
        elif filter_type == 3:
            for i in range(stride):
                left = row[i - 4] if i >= 4 else 0
                row[i] = (row[i] + ((left + prev[i]) // 2)) & 0xFF
        elif filter_type == 4:
            for i in range(stride):
                left = row[i - 4] if i >= 4 else 0
                up_left = prev[i - 4] if i >= 4 else 0
                row[i] = (row[i] + paeth(left, prev[i], up_left)) & 0xFF
        else:
            raise SystemExit(f"不认识的 PNG 滤波器 {filter_type}")
        rows.append(row)
    if src != len(raw):
        raise SystemExit("PNG 像素长度不对")
    return width, height, b"".join(rows)


def downscale(src: bytes, src_w: int, src_h: int, dst_w: int, dst_h: int) -> bytes:
    """预乘 alpha 后的整数面积平均。权重是有理数，四舍五入到整数。"""
    out = bytearray(dst_w * dst_h * 4)
    for y in range(dst_h):
        y0_num = y * src_h
        y1_num = (y + 1) * src_h
        for x in range(dst_w):
            x0_num = x * src_w
            x1_num = (x + 1) * src_w
            acc_r = acc_g = acc_b = acc_a = 0
            sy = y0_num // dst_h
            while sy * dst_h < y1_num and sy < src_h:
                top = sy * dst_h
                bot = (sy + 1) * dst_h
                overlap_y = min(y1_num, bot) - max(y0_num, top)
                if overlap_y > 0:
                    sx = x0_num // dst_w
                    row = sy * src_w
                    while sx * dst_w < x1_num and sx < src_w:
                        left = sx * dst_w
                        right = (sx + 1) * dst_w
                        overlap_x = min(x1_num, right) - max(x0_num, left)
                        if overlap_x > 0:
                            weight = overlap_x * overlap_y
                            i = (row + sx) * 4
                            red, green, blue, alpha = src[i : i + 4]
                            acc_r += red * alpha * weight
                            acc_g += green * alpha * weight
                            acc_b += blue * alpha * weight
                            acc_a += alpha * weight
                        sx += 1
                sy += 1
            area = src_w * src_h
            if acc_a == 0:
                continue
            alpha = (acc_a + area // 2) // area
            red = (acc_r + acc_a // 2) // acc_a
            green = (acc_g + acc_a // 2) // acc_a
            blue = (acc_b + acc_a // 2) // acc_a
            o = (y * dst_w + x) * 4
            out[o : o + 4] = bytes((red, green, blue, alpha))
    return bytes(out)


def zlib_store(data: bytes) -> bytes:
    out = bytearray(b"\x78\x01")
    pos = 0
    while True:
        chunk = data[pos : pos + 65535]
        last = pos + len(chunk) >= len(data)
        out.append(1 if last else 0)
        out += len(chunk).to_bytes(2, "little")
        out += (len(chunk) ^ 0xFFFF).to_bytes(2, "little")
        out += chunk
        if last:
            break
        pos += len(chunk)
    out += zlib.adler32(data).to_bytes(4, "big")
    stream = bytes(out)
    if zlib.decompress(stream) != data:
        raise SystemExit("未压缩 zlib 块解不开")
    return stream


def chunk(kind: bytes, data: bytes) -> bytes:
    crc = zlib.crc32(kind + data) & 0xFFFFFFFF
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", crc)


def encode_png(width: int, height: int, rgba: bytes) -> bytes:
    stride = width * 4
    raw = bytearray()
    for y in range(height):
        raw.append(0)
        raw += rgba[y * stride : (y + 1) * stride]
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib_store(bytes(raw))) + chunk(b"IEND", b"")


def and_mask(rgba: bytes, width: int, height: int) -> bytes:
    row_bytes = ((width + 31) // 32) * 4
    mask = bytearray(row_bytes * height)
    for y in range(height):
        src_y = height - 1 - y
        for x in range(width):
            alpha = rgba[(src_y * width + x) * 4 + 3]
            if alpha < 128:
                mask[y * row_bytes + x // 8] |= 0x80 >> (x % 8)
    return bytes(mask)


def dib(rgba: bytes, width: int, height: int) -> bytes:
    xor = bytearray(width * height * 4)
    for y in range(height):
        src_y = height - 1 - y
        for x in range(width):
            i = (src_y * width + x) * 4
            red, green, blue, alpha = rgba[i : i + 4]
            o = (y * width + x) * 4
            xor[o : o + 4] = bytes((blue, green, red, alpha))
    mask = and_mask(rgba, width, height)
    header = struct.pack(
        "<IiiHHIIiiII",
        40,
        width,
        height * 2,
        1,
        32,
        0,
        len(xor) + len(mask),
        0,
        0,
        0,
        0,
    )
    return header + bytes(xor) + mask


def encode_ico(images: list[tuple[int, bytes]]) -> bytes:
    header = struct.pack("<HHH", 0, 1, len(images))
    entries = bytearray()
    blobs = bytearray()
    offset = 6 + 16 * len(images)
    for width, rgba in images:
        image = dib(rgba, width, width)
        entries += struct.pack(
            "<BBBBHHII",
            width % 256,
            width % 256,
            0,
            0,
            1,
            32,
            len(image),
            offset,
        )
        blobs += image
        offset += len(image)
    return header + bytes(entries) + bytes(blobs)


def main() -> None:
    svg = SVG_PATH.read_bytes()
    png = PNG_256.read_bytes()
    if sha256(svg) != SVG_SHA256:
        raise SystemExit("lanwork.svg 的字节变了。图形要求原样使用，不要改花瓣或颜色。")
    if sha256(png) != PNG_256_SHA256:
        raise SystemExit("lanwork-256.png 的字节变了。这是用户给出的 256px 渲染，不要重写。")
    width, height, base = decode_png(png)
    images: list[tuple[int, bytes]] = []
    for size in SIZES:
        if size == 256:
            rgba = base
        else:
            rgba = downscale(base, width, height, size, size)
            (ASSET_DIR / f"lanwork-{size}.png").write_bytes(encode_png(size, size, rgba))
        images.append((size, rgba))
        if size == 32:
            (ASSET_DIR / "lanwork-32.rgba").write_bytes(rgba)
    (ASSET_DIR / "lanwork.ico").write_bytes(encode_ico(images))
    print("wrote", ", ".join(str(size) for size in SIZES))


if __name__ == "__main__":
    main()
