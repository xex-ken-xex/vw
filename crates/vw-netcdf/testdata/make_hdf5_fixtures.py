"""vw-netcdf のテストに使う、NetCDF-4 の印のない小さい HDF5 のファイルを作る。

`python3 make_hdf5_fixtures.py [出力フォルダ]`（既定はこのスクリプトのあるフォルダ）。h5py・numpy が要る。
値は式で決まる作り物で、テストは同じ式で期待値を出す（期待値のファイルはない）。

hdf5_cases.h5:
- HDFEOS/SWATHS/S/Data Fields/Radiance: uint16 4 × 5。値は 1000 + 10 行 + 列、scale_factor 0.01、
  左上だけ _FillValue（65535）
- HDFEOS/SWATHS/S/Data Fields/CloudFlag: uint8 4 × 5。値は (行 + 列) % 2、flag_values と flag_meanings（clear cloudy）
- HDFEOS/SWATHS/S/Geolocation Fields/Latitude・Longitude: float32 4 × 5。緯度 30 + 行、経度 130 + 列。
  units は "degrees" だけ（名前で緯度・経度と見分ける）。経度の右下だけ欠損の -999
- lzf_flags: uint8 16 × 16、LZF 圧縮（チャンク 8 × 8）。値は (16 行 + 列) % 7
- cube: float32 3 × 4 × 5。値は 100 面 + 5 行 + 列
- kind: 列挙型（uint8）4 × 5、obs: 構造体の 1 次元（どちらも画像にしない）
"""

import sys
from pathlib import Path

import h5py
import numpy as np

out = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent
out.mkdir(parents=True, exist_ok=True)

rows, cols = np.meshgrid(np.arange(4), np.arange(5), indexing="ij")
with h5py.File(out / "hdf5_cases.h5", "w", track_order=False) as f:
    swath = f.create_group("HDFEOS/SWATHS/S")
    data = swath.create_group("Data Fields")
    radiance = (1000 + 10 * rows + cols).astype("u2")
    radiance[0, 0] = 65535
    r = data.create_dataset("Radiance", data=radiance)
    r.attrs["scale_factor"] = np.float64(0.01)
    r.attrs["_FillValue"] = np.uint16(65535)
    r.attrs["units"] = "W m-2 sr-1 um-1"
    flag = data.create_dataset("CloudFlag", data=((rows + cols) % 2).astype("u1"))
    flag.attrs["flag_values"] = np.array([0, 1], "u1")
    flag.attrs["flag_meanings"] = "clear cloudy"
    geo = swath.create_group("Geolocation Fields")
    lat = geo.create_dataset("Latitude", data=(30.0 + rows).astype("f4"))
    lat.attrs["units"] = "degrees"
    lon = (130.0 + cols).astype("f4")
    lon[3, 4] = -999.0
    geo.create_dataset("Longitude", data=lon).attrs["units"] = "degrees"

    r16, c16 = np.meshgrid(np.arange(16), np.arange(16), indexing="ij")
    f.create_dataset("lzf_flags", data=((16 * r16 + c16) % 7).astype("u1"),
                     chunks=(8, 8), compression="lzf")

    k, r3, c3 = np.meshgrid(np.arange(3), np.arange(4), np.arange(5), indexing="ij")
    f.create_dataset("cube", data=(100 * k + 5 * r3 + c3).astype("f4"))

    kind = h5py.enum_dtype({"water": 0, "land": 1}, basetype="u1")
    f.create_dataset("kind", data=((rows + cols) % 2).astype("u1"), dtype=kind)
    obs = np.zeros(3, dtype=[("t", "f8"), ("v", "i4")])
    f.create_dataset("obs", data=obs)
print(out / "hdf5_cases.h5")
