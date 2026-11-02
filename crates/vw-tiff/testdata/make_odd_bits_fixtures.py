"""vw-tiff のテストに使う、1 サンプルが 10・12・14・24bit の小さい TIFF を作る。

`python3 make_odd_bits_fixtures.py [出力フォルダ]`（既定はこのスクリプトのあるフォルダ）。
numpy・imagecodecs（LZW・PackBits の圧縮）・tifffile（読み返しの確かめ）が要る。

値は式で決まる: 画素 (x, y) の c 番目のサンプルは (x * 37 + y * 101 + c * 13 + 5) mod 2^bits。
WhiteIsZero でも書く値は同じ（読む側が反転する）。符号付きは tifffile が読めないので、ここでは作らずテストの中で作る。
テスト（src/tests.rs）は同じ式で期待値を作る。

詰め方は TIFF の決まりどおり上位ビットから。行（タイルなら タイルの行）の終わりは 1 バイトに切り上げる。
作ったファイルは tifffile（libtiff と同じ詰め方で読む、vw とは別の実装）で読み返し、値が式と同じことを確かめる。
"""

import struct
import sys
import zlib
from pathlib import Path

import imagecodecs
import numpy as np
import tifffile

out = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent
out.mkdir(parents=True, exist_ok=True)


def values(width, height, samples, bits, signed):
    y, x, c = np.meshgrid(np.arange(height), np.arange(width), np.arange(samples), indexing="ij")
    v = (x * 37 + y * 101 + c * 13 + 5) % (1 << bits)
    return v.astype(np.int64) - (1 << (bits - 1)) if signed else v.astype(np.int64)


def pack(row, bits):
    """整数の並びを、上位ビットから bits ビットずつ詰める（末尾は 1 バイトに切り上げ）。"""
    acc = 0
    for v in row:
        acc = (acc << bits) | (int(v) & ((1 << bits) - 1))
    n = len(row) * bits
    pad = (-n) % 8
    return (acc << pad).to_bytes((n + pad) // 8, "big")


COMPRESS = {
    1: lambda b: b,
    5: imagecodecs.lzw_encode,
    8: zlib.compress,
    32773: imagecodecs.packbits_encode,
}


def write(name, width, height, samples, bits, photometric, *, planar=False, compression=1,
          tile=None, rows_per_strip=None, big_endian=False, extra=None, signed=False):
    arr = values(width, height, samples, bits, signed)
    planes = [arr[:, :, c : c + 1] for c in range(samples)] if planar else [arr]
    chunks = []
    for plane in planes:
        s = plane.shape[2]
        if tile:
            tw, th = tile
            for ty in range(0, height, th):
                for tx in range(0, width, tw):
                    rows = []
                    for y in range(ty, ty + th):
                        row = []
                        for x in range(tx, tx + tw):
                            inside = y < height and x < width
                            row.extend(plane[y, x] if inside else [0] * s)
                        rows.append(pack(row, bits))
                    chunks.append(b"".join(rows))
        else:
            rps = rows_per_strip or height
            for y0 in range(0, height, rps):
                rows = [pack(plane[y].reshape(-1), bits) for y in range(y0, min(height, y0 + rps))]
                chunks.append(b"".join(rows))
    chunks = [COMPRESS[compression](c) for c in chunks]

    e = ">" if big_endian else "<"
    data = bytearray(b"MM\0*" if big_endian else b"II*\0")
    data += struct.pack(e + "I", 0)
    offsets = []
    for c in chunks:
        offsets.append(len(data))
        data += c
        if len(data) % 2:
            data += b"\0"
    tags = {
        256: (4, [width]),
        257: (4, [height]),
        258: (3, [bits] * samples),
        259: (3, [compression]),
        262: (3, [photometric]),
        277: (3, [samples]),
        284: (3, [2 if planar else 1]),
    }
    if tile:
        tags[322] = (4, [tile[0]])
        tags[323] = (4, [tile[1]])
        tags[324] = (4, offsets)
        tags[325] = (4, [len(c) for c in chunks])
    else:
        tags[273] = (4, offsets)
        tags[278] = (4, [rows_per_strip or height])
        tags[279] = (4, [len(c) for c in chunks])
    if extra is not None:
        tags[338] = (3, [extra])
    if signed:
        tags[339] = (3, [2] * samples)
    ifd = len(data)
    struct.pack_into(e + "I", data, 4, ifd)
    extra_at = ifd + 2 + 12 * len(tags) + 4
    entries, tail = b"", b""
    for tag in sorted(tags):
        kind, vals = tags[tag]
        raw = b"".join(struct.pack(e + ("H" if kind == 3 else "I"), v) for v in vals)
        entry = struct.pack(e + "HHI", tag, kind, len(vals))
        if len(raw) <= 4:
            entry += raw.ljust(4, b"\0")
        else:
            entry += struct.pack(e + "I", extra_at + len(tail))
            tail += raw + (b"\0" if len(raw) % 2 else b"")
        entries += entry
    data += struct.pack(e + "H", len(tags)) + entries + struct.pack(e + "I", 0) + tail
    path = out / name
    path.write_bytes(bytes(data))

    back = tifffile.imread(path)
    want = arr if samples > 1 else arr[:, :, 0]
    if planar and back.shape[0] == samples:
        back = np.moveaxis(back, 0, -1)
    assert back.shape == want.shape, (name, back.shape, want.shape)
    assert (back.astype(np.int64) == want).all(), name
    print(name, len(data), "bytes")


write("gray12_strips_le.tif", 13, 7, 1, 12, 1, rows_per_strip=3)
write("gray12_lzw_be.tif", 13, 7, 1, 12, 1, compression=5, rows_per_strip=2, big_endian=True)
write("gray10_deflate_tiles.tif", 21, 18, 1, 10, 1, compression=8, tile=(16, 16))
write("gray14_packbits.tif", 9, 5, 1, 14, 1, compression=32773)
write("gray24_deflate.tif", 11, 4, 1, 24, 1, compression=8)
write("gray12_white_is_zero.tif", 5, 3, 1, 12, 0)
write("rgb12_lzw.tif", 7, 5, 3, 12, 2, compression=5, rows_per_strip=2)
write("rgb12_planar_deflate.tif", 7, 5, 3, 12, 2, planar=True, compression=8, rows_per_strip=2)
write("rgba10_planar_tiles.tif", 19, 17, 4, 10, 2, planar=True, tile=(16, 16), extra=2)
write("rgba14_be.tif", 6, 3, 4, 14, 2, big_endian=True, extra=2)
