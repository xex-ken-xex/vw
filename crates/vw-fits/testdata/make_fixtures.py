"""vw-fits のテストが読む tile 圧縮の FITS と、その答え合わせ用のファイルを作る。

`python3 make_fixtures.py`（このフォルダで）。astropy と numpy が要る。圧縮した `.fz` と、
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
