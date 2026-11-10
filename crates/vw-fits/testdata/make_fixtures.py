"""vw-fits のテストが読む tile 圧縮の FITS と、その答え合わせ用のファイルを作る。

`python3 make_fixtures.py`（このフォルダで）。astropy と numpy が要る（HCOMPRESS・PLIO も astropy が圧縮する）。圧縮した `.fz` と、
同じ画素を圧縮せずに書いた `.fits`（実数の量子化は astropy が展開した値）を対にして置く。
"""

import numpy as np
from astropy.io import fits

rng = np.random.default_rng(7)
H, W = 17, 25

# 16bit（BZERO=32768）の 2 面。tile は 5 行ずつなので、下の端の tile は 2 行になる。
u16 = rng.normal(30000, 8000, (2, H, W)).clip(0, 65535).astype(np.uint16)
fits.PrimaryHDU(u16).writeto("uint16-cube.fits", overwrite=True)
for comp, name in [("RICE_1", "rice"), ("GZIP_1", "gzip1"), ("GZIP_2", "gzip2")]:
    c = fits.CompImageHDU(u16, compression_type=comp, tile_shape=(1, 5, W))
    fits.HDUList([fits.PrimaryHDU(), c]).writeto(f"uint16-cube-{name}.fz", overwrite=True)

i32 = rng.integers(-(2**30), 2**30, (H, W)).astype(np.int32)
fits.PrimaryHDU(i32).writeto("int32.fits", overwrite=True)
fits.HDUList([fits.PrimaryHDU(), fits.CompImageHDU(i32, compression_type="RICE_1")]).writeto(
    "int32-rice.fz", overwrite=True)

u8 = rng.integers(0, 256, (H, W)).astype(np.uint8)
fits.PrimaryHDU(u8).writeto("uint8.fits", overwrite=True)
fits.HDUList([fits.PrimaryHDU(), fits.CompImageHDU(u8, compression_type="RICE_1")]).writeto(
    "uint8-rice.fz", overwrite=True)

f32 = rng.normal(0, 1, (H, W)).astype(np.float32)
f32[3, 4] = np.nan
for method, name in [(1, "dither1"), (2, "dither2"), (-1, "nodither")]:
    c = fits.CompImageHDU(f32, compression_type="RICE_1", quantize_method=method)
    fits.HDUList([fits.PrimaryHDU(), c]).writeto(f"float32-rice-{name}.fz", overwrite=True)
    back = fits.open(f"float32-rice-{name}.fz")[1].data
    fits.PrimaryHDU(back.astype(np.float32)).writeto(
        f"float32-rice-{name}-expected.fits", overwrite=True)

# HCOMPRESS_1（astropy の中の CFITSIO が圧縮する）。scale 0 は可逆。tile の幅と高さが 2 の累乗でない
# 17 x 25 を、端の tile が半端になる 6 x 10 の tile で（astropy は端の tile が 4 画素より細いと断る）。
for name, data in [("uint16", u16[0]), ("int32", i32 >> 8), ("uint8", u8)]:
    c = fits.CompImageHDU(data, compression_type="HCOMPRESS_1", tile_shape=(6, 10))
    fits.HDUList([fits.PrimaryHDU(), c]).writeto(f"{name}-hcompress.fz", overwrite=True)
fits.PrimaryHDU(i32 >> 8).writeto("int32-shifted.fits", overwrite=True)

# 非可逆の HCOMPRESS（scale 4）と、ならし（smooth）あり。答えは astropy が展開した値。
smooth_src = (np.add.outer(np.arange(H) * 300, np.arange(W) * 200) + rng.normal(0, 50, (H, W))).astype(np.int16)
for smooth in (0, 1):
    c = fits.CompImageHDU(smooth_src, compression_type="HCOMPRESS_1", tile_shape=(H, W),
                          hcomp_scale=4, hcomp_smooth=smooth)
    name = f"int16-hcompress-scale4-smooth{smooth}"
    fits.HDUList([fits.PrimaryHDU(), c]).writeto(f"{name}.fz", overwrite=True)
    back = fits.open(f"{name}.fz")[1].data
    fits.PrimaryHDU(back.astype(np.int16)).writeto(f"{name}-expected.fits", overwrite=True)

# 量子化した実数の HCOMPRESS。
c = fits.CompImageHDU(f32, compression_type="HCOMPRESS_1", tile_shape=(H, W), quantize_method=1)
fits.HDUList([fits.PrimaryHDU(), c]).writeto("float32-hcompress.fz", overwrite=True)
back = fits.open("float32-hcompress.fz")[1].data
fits.PrimaryHDU(back.astype(np.float32)).writeto("float32-hcompress-expected.fits", overwrite=True)

# PLIO_1（マスクのような、同じ値が続く 0 以上の整数）。
mask = np.zeros((H, W), np.int32)
mask[2:9, 3:20] = 7
mask[10:15, 5:8] = 300
mask[12, :] = 70000
mask[16, ::3] = 1
fits.PrimaryHDU(mask).writeto("mask.fits", overwrite=True)
c = fits.CompImageHDU(mask, compression_type="PLIO_1")
fits.HDUList([fits.PrimaryHDU(), c]).writeto("mask-plio.fz", overwrite=True)
