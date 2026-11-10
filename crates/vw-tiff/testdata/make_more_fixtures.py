"""vw-tiff のテストに使う、tiff クレートが読まない形の小さい TIFF を作る（TODO #45 の残り）。

`python3 make_more_fixtures.py [出力フォルダ]`（既定はこのスクリプトのあるフォルダ）。
numpy・tifffile・imagecodecs・Pillow（libtiff 付き）と、libtiff の tiffcp・tiff2rgba、libtiff.so が要る
（Ubuntu なら `apt install libtiff-tools`、`pip install numpy tifffile imagecodecs pillow`）。

作るもの（テスト src/tests.rs の「TIFF の残りの形」で使う）:

- FAX: 同じ白黒の絵を、無圧縮（BlackIsZero と WhiteIsZero の 2 通り）、Huffman（2）、語単位の Huffman（32771）、
  T.4 の 1 次元・2 次元・EOL をバイトにそろえたもの・FillOrder = 2 で。圧縮は libtiff が行う。
- LZMA・ZSTD（差分つきも）・WebP: 4 色の絵（左上 赤、右上 緑、左下 青、右下 白）を libtiff で圧縮。
- L*a*b*: 同じ 4 色を CIELab（8・16bit）、ICCLab（8・16bit）、ITULab（8bit）で。
- LogLuv・LogL: libtiff（ctypes で呼ぶ）に実数の XYZ を渡して SGILog で書く。灰色と赤の 2 面、
  LogL は明るさ 0.5・1・100・1000 の 4 面。
- 間引いた YCbCr: 4 色の絵（40x24）を 2x2・4x2・2x1 の無圧縮で。幅と高さが割り切れない 37x23 は LZW で
  （tiffcp は間引いた画像を写せないので、並べるのも LZW の圧縮もここで行う）。
  tiff2rgba で読み返して 4 色に戻ることを確かめる（並べ方の誤りを libtiff で見つけるため）。
- 多バンド: 5 バンドの 16bit（並べた形と面を分けた形）、6 バンドの RGB、アルファ付きのグレー、CMYK の実数。
"""

import ctypes
import subprocess
import sys
import tempfile
from pathlib import Path

import imagecodecs
import numpy as np
import tifffile
from PIL import Image, ImageDraw

out = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent
out.mkdir(parents=True, exist_ok=True)
work = Path(tempfile.mkdtemp())

QUADS = [(255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 255)]


def quadrants(width, height):
    img = np.zeros((height, width, 3), np.uint8)
    hw, hh = width // 2, height // 2
    img[:hh, :hw] = QUADS[0]
    img[:hh, hw:] = QUADS[1]
    img[hh:, :hw] = QUADS[2]
    img[hh:, hw:] = QUADS[3]
    return img


def tiffcp(args, src, dst):
    subprocess.run(["tiffcp", *args, str(src), str(out / dst)], check=True)


def rgba_by_libtiff(path):
    """libtiff の TIFFReadRGBAImage で読んだ画素（tiff2rgba の出力）。"""
    dst = work / (Path(path).stem + "-rgba.tif")
    subprocess.run(["tiff2rgba", "-c", "none", str(path), str(dst)], check=True)
    return tifffile.imread(dst)[..., :3]


def assert_quadrants(img, width, height, tolerance, name):
    hw, hh = width // 2, height // 2
    for (y, x), want in zip([(0, 0), (0, hw), (hh, 0), (hh, hw)], QUADS):
        got = img[y + 1, x + 1].astype(int)
        assert np.all(np.abs(got - want) <= tolerance), (name, (x, y), got, want)


# --- FAX -------------------------------------------------------------------
bw = Image.new("1", (61, 23), 1)
draw = ImageDraw.Draw(bw)
draw.ellipse([2, 2, 30, 20], fill=0)
draw.rectangle([34, 3, 58, 9], fill=0)
for x in range(34, 60, 3):
    draw.line([x, 12, x, 21], fill=0)
black = (np.array(bw) == 0)
# 黒が 1 の WhiteIsZero（FAX のふつうの書き方）と、黒が 0 の BlackIsZero で、同じ絵。
tifffile.imwrite(work / "bw_white.tif", black, photometric="miniswhite", rowsperstrip=8)
tifffile.imwrite(out / "fax_none_white_is_zero.tif", black, photometric="miniswhite")
bw.save(out / "fax_none_black_is_zero.tif")
bw.save(out / "fax_huffman.tif", compression="tiff_ccitt")
bw.save(out / "fax_huffman_word.tif", compression="tiff_raw_16")
tiffcp(["-c", "g3:1d", "-r", "8"], work / "bw_white.tif", "fax_g3_1d.tif")
tiffcp(["-c", "g3:2d", "-r", "8"], work / "bw_white.tif", "fax_g3_2d.tif")
tiffcp(["-c", "g3:2d:fill", "-r", "8"], work / "bw_white.tif", "fax_g3_2d_fill.tif")
tiffcp(["-c", "g3:2d", "-f", "lsb2msb", "-r", "8"], work / "bw_white.tif", "fax_g3_2d_lsb.tif")

# --- LZMA・ZSTD・WebP -------------------------------------------------------
tifffile.imwrite(work / "rgb.tif", quadrants(36, 24), photometric="rgb")
tifffile.imwrite(out / "rgb_none.tif", quadrants(36, 24), photometric="rgb")
tiffcp(["-c", "lzma", "-r", "8"], work / "rgb.tif", "rgb_lzma.tif")
tiffcp(["-c", "zstd", "-r", "8"], work / "rgb.tif", "rgb_zstd.tif")
tiffcp(["-c", "zstd:2", "-t", "-w", "16", "-l", "16"], work / "rgb.tif", "rgb_zstd_predictor_tiles.tif")
tiffcp(["-c", "webp", "-r", "8"], work / "rgb.tif", "rgb_webp.tif")


# --- L*a*b* ----------------------------------------------------------------
def srgb_to_lab(rgb):
    c = np.asarray(rgb, float) / 255
    lin = np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)
    # sRGB（D65）から XYZ（D50）へ。Bradford で白を合わせた行列。
    m = np.array([[0.4360747, 0.3850649, 0.1430804],
                  [0.2225045, 0.7168786, 0.0606169],
                  [0.0139322, 0.0971045, 0.7141733]])
    x, y, z = m @ lin / np.array([0.9642, 1.0, 0.8249])
    f = lambda t: np.cbrt(t) if t > (6 / 29) ** 3 else t / (3 * (6 / 29) ** 2) + 4 / 29
    fx, fy, fz = f(x), f(y), f(z)
    return 116 * fy - 16, 500 * (fx - fy), 200 * (fy - fz)


def lab_image(encode, dtype):
    img = quadrants(36, 24)
    lab = {tuple(c): encode(*srgb_to_lab(c)) for c in QUADS}
    return np.array([[lab[tuple(p)] for p in row] for row in img], dtype)


cielab8 = lab_image(lambda L, a, b: (round(L * 255 / 100), round(a) & 0xFF, round(b) & 0xFF), np.uint8)
tifffile.imwrite(out / "lab_cielab8.tif", cielab8, photometric="cielab")
# libtiff の Lab の読み込みは表示用の近似（白を D65 として扱う）で、緑が大きくずれるので比べに使わない。
# 代わりに、広く使われる値（sRGB の赤 = L*a*b*（D50）の 54.29, 80.80, 69.89）と合うことを確かめる。
assert np.allclose(srgb_to_lab((255, 0, 0)), (54.29, 80.80, 69.89), atol=0.05), srgb_to_lab((255, 0, 0))
tifffile.imwrite(out / "lab_cielab16.tif", lab_image(
    lambda L, a, b: (round(L * 65535 / 100), round(a * 256) & 0xFFFF, round(b * 256) & 0xFFFF),
    np.uint16), photometric="cielab")
tifffile.imwrite(out / "lab_icclab8.tif", lab_image(
    lambda L, a, b: (round(L * 255 / 100), round(a + 128), round(b + 128)), np.uint8),
    photometric="icclab")
tifffile.imwrite(out / "lab_icclab16.tif", lab_image(
    lambda L, a, b: (round(L * 65535 / 100), round((a + 128) * 257), round((b + 128) * 257)),
    np.uint16), photometric="icclab")
tifffile.imwrite(out / "lab_itulab8.tif", lab_image(
    # ITULab の範囲（a* −85〜85、b* −75〜125）に入らない青と緑は端に寄る。テストは赤と白だけを見る。
    lambda L, a, b: (round(L * 255 / 100), min(max(round((a + 85) * 255 / 170), 0), 255),
                     min(max(round((b + 75) * 255 / 200), 0), 255)),
    np.uint8), photometric="itulab")

# --- LogLuv・LogL（libtiff に書かせる）--------------------------------------
lib = ctypes.CDLL("libtiff.so.6")
lib.TIFFOpen.restype = ctypes.c_void_p
lib.TIFFOpen.argtypes = [ctypes.c_char_p, ctypes.c_char_p]
lib.TIFFWriteScanline.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint16]
lib.TIFFClose.argtypes = [ctypes.c_void_p]
TAGS = dict(width=256, length=257, bits=258, compression=259, photometric=262, samples=277,
            rows=278, planar=284, sampleformat=339, sgilogdatafmt=65560)


def set_field(tif, tag, value):
    lib.TIFFSetField.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_int]
    assert lib.TIFFSetField(tif, TAGS[tag], value) == 1, tag


def write_sgilog(path, pixels, photometric):
    """`pixels`（高さ × 幅 × サンプル数の実数。LogLuv は XYZ、LogL は Y）を SGILog で書く。"""
    height, width, samples = pixels.shape
    tif = lib.TIFFOpen(str(path).encode(), b"w")
    for tag, value in [("width", width), ("length", height), ("samples", samples), ("bits", 32),
                       ("photometric", photometric), ("compression", 34676), ("planar", 1),
                       ("sampleformat", 3), ("sgilogdatafmt", 0), ("rows", 4)]:
        set_field(tif, tag, value)
    for y in range(height):
        row = np.ascontiguousarray(pixels[y], np.float32)
        assert lib.TIFFWriteScanline(tif, row.ctypes.data, y, 0) == 1
    lib.TIFFClose(tif)


def xyz_of_linear_srgb(rgb):
    m = np.array([[0.4124, 0.3576, 0.1805], [0.2126, 0.7152, 0.0722], [0.0193, 0.1192, 0.9505]])
    return m @ np.asarray(rgb, float)


luv = np.zeros((8, 12, 3), np.float32)
luv[:, :6] = np.array([1 / 3, 1 / 3, 1 / 3]) / (1 / 3) * 50  # 等エネルギーの白（x = y = 1/3）、Y = 50
luv[:, 6:] = xyz_of_linear_srgb([20, 0, 0])
write_sgilog(out / "logluv.tif", luv, 32845)
logl = np.zeros((8, 8, 1), np.float32)
for i, y in enumerate([0.5, 1, 100, 1000]):
    logl[:, i * 2:(i + 1) * 2] = y
write_sgilog(out / "logl.tif", logl, 32844)


# --- 間引いた YCbCr --------------------------------------------------------
def ycbcr_of(rgb):
    r, g, b = (np.asarray(rgb, float)[..., i] for i in range(3))
    y = 0.299 * r + 0.587 * g + 0.114 * b
    return y, (b - y) / 1.772 + 128, (r - y) / 1.402 + 128


def write_subsampled(path, width, height, h, v, rows_per_strip, lzw=False):
    """libtiff の決まりどおり、h × v 画素の塊ごとに Y を h × v 個、Cb、Cr の順で並べた無圧縮の TIFF。"""
    img = quadrants(width, height).astype(float)
    pw, ph = -(-width // h) * h, -(-height // v) * v
    pad = np.pad(img, ((0, ph - height), (0, pw - width), (0, 0)), mode="edge")
    y, cb, cr = ycbcr_of(pad)
    strips = []
    for top in range(0, ph, rows_per_strip):
        data = bytearray()
        for by in range(top, min(top + rows_per_strip, ph), v):
            for bx in range(0, pw, h):
                byte = lambda x: min(max(int(round(x)), 0), 255)
                data += bytes(byte(y[by + j, bx + i]) for j in range(v) for i in range(h))
                data += bytes([byte(cb[by:by + v, bx:bx + h].mean()),
                               byte(cr[by:by + v, bx:bx + h].mean())])
        strips.append(imagecodecs.lzw_encode(bytes(data)) if lzw else bytes(data))
    write_raw_tiff(path, [
        (256, 4, [width]), (257, 4, [height]), (258, 3, [8, 8, 8]), (259, 3, [5 if lzw else 1]), (262, 3, [6]),
        (277, 3, [3]), (278, 4, [rows_per_strip]), (284, 3, [1]), (530, 3, [h, v]),
        (532, 5, [0, 1, 255, 1, 128, 1, 255, 1, 128, 1, 255, 1]),
    ], strips)


def write_raw_tiff(path, tags, strips):
    """リトルエンディアンの TIFF を、並べ済みのストリップ `strips` と `tags`（番号, 型, 値）から書く。
    StripOffsets と StripByteCounts はここで足す。tifffile は間引いた YCbCr の並びを自分では作らないため。"""
    import struct
    body = bytearray(b"II*\0\0\0\0\0")
    offsets = []
    for strip in strips:
        offsets.append(len(body))
        body += strip
        if len(body) % 2:
            body += b"\0"
    tags = sorted(tags + [(273, 4, offsets), (279, 4, [len(x) for x in strips])])
    ifd = len(body)
    struct.pack_into("<I", body, 4, ifd)
    extra_at = ifd + 2 + 12 * len(tags) + 4
    entries, extra = bytearray(struct.pack("<H", len(tags))), bytearray()
    for tag, kind, values in tags:
        fmt = {3: "H", 4: "I", 5: "I"}[kind]
        data = struct.pack("<" + fmt * len(values), *values)
        count = len(values) // 2 if kind == 5 else len(values)
        entries += struct.pack("<HHI", tag, kind, count)
        if len(data) <= 4:
            entries += data.ljust(4, b"\0")
        else:
            entries += struct.pack("<I", extra_at + len(extra))
            extra += data
    body += entries + b"\0\0\0\0" + extra
    Path(path).write_bytes(bytes(body))


for (h, v) in [(2, 2), (4, 2), (2, 1)]:
    name = f"ycbcr_{h}x{v}.tif"
    write_subsampled(out / name, 40, 24, h, v, 8)
    assert_quadrants(rgba_by_libtiff(out / name), 40, 24, 3, name)
write_subsampled(out / "ycbcr_2x2_odd_lzw.tif", 37, 23, 2, 2, 6, lzw=True)
assert_quadrants(rgba_by_libtiff(out / "ycbcr_2x2_odd_lzw.tif"), 37, 23, 3, "odd")


# --- 多バンド --------------------------------------------------------------
def bands(width, height, count):
    yy, xx, cc = np.meshgrid(np.arange(height), np.arange(width), np.arange(count), indexing="ij")
    return ((xx * 37 + yy * 101 + cc * 1000 + 5) % 65536).astype(np.uint16)


tifffile.imwrite(out / "bands5.tif", bands(9, 7, 5), photometric="minisblack", planarconfig="contig")
tifffile.imwrite(out / "bands5_planar.tif", np.moveaxis(bands(9, 7, 5), 2, 0),
                 photometric="minisblack", planarconfig="separate")
tifffile.imwrite(out / "bands6_rgb.tif", bands(9, 7, 6), photometric="rgb", planarconfig="contig",
                 extrasamples=[0, 0, 0])
gray_alpha = np.stack([np.full((7, 9), 200, np.uint8), np.full((7, 9), 100, np.uint8)], axis=2)
tifffile.imwrite(out / "gray_alpha.tif", gray_alpha, photometric="minisblack", extrasamples=["unassalpha"])
cmyk = np.zeros((7, 9, 4), np.float32)
cmyk[..., 0] = 1.0  # シアン 100%
cmyk[..., 3] = 0.5  # 墨 50%
tifffile.imwrite(out / "cmyk_float.tif", cmyk, photometric="separated")

print("wrote", out)
