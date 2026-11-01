"""vw-netcdf のテストに使う、小さい NetCDF のファイルと期待値を作る。

`python3 make_fixtures.py [出力フォルダ]`（既定はこのスクリプトのあるフォルダ）。netCDF4・numpy・pyproj が要る。
値はどれも作り物。手で見る確認用の大きい版（tools/make_test_hdf5_netcdf.py）と同じ書き方を、
格子を 5 度ごとに縮めて作る。乱数の種も同じ。

- era5_like.nc: NetCDF-4。時刻 2 × 緯度 37 × 経度 72。int16 の気温を scale_factor・add_offset で戻す。
  一部が _FillValue。緯度は北から南へ並ぶ
- classic.nc / classic_64bit.nc: NetCDF-3 の CDF-1 と CDF-2。気温と陸と海の印（flag_values）
- sst_groups.nc: NetCDF-4 のグループと無制限の次元。陸は _FillValue、_Unsigned の byte
- geo_cases.nc: NetCDF-4。座標の読み方を確かめる小さい格子。経度 0〜360°、曲がった格子（画素ごとの
  緯度・経度の表）、ランベルト図法（x・y は km）、回転極、偏差の変数。期待値は geo-expected.txt
  （図法の分は PROJ（pyproj）で x・y から緯度・経度に直した値）
- edge_cases.nc: NetCDF-3。緯度が南から北へ並ぶ変数、(経度, 緯度) の順の変数、
  計測の値の valid_range と格納値の valid_range

expected-values.txt に、netCDF4 で読んだ（scale_factor と欠損を当てた）ときの形・欠損の数・
最小・最大・平均と、最初の面の左上と右下の値を書く。measured_range だけは、netCDF4 と違い
valid_range を計測の値と比べた値（vw と netCDF-Java の読み方）にする。
"""

import sys
from pathlib import Path

import netCDF4
import numpy as np

out = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent
out.mkdir(parents=True, exist_ok=True)
rng = np.random.default_rng(20261101)


def land_mask(lat, lon):
    la, lo = np.meshgrid(lat, lon, indexing="ij")
    m = np.zeros(la.shape, bool)
    for clat, clon, rlat, rlon in [(45, 90, 25, 50), (10, 20, 30, 20), (40, -100, 20, 30),
                                   (-15, -60, 20, 15), (-25, 135, 12, 20), (-80, 0, 10, 360)]:
        d = ((la - clat) / rlat) ** 2 + (((lo - clon + 180) % 360 - 180) / rlon) ** 2
        m |= d < 1
    return m


def temperature(lat, lon, hour):
    la, lo = np.meshgrid(lat, lon, indexing="ij")
    base = 288 - 40 * np.sin(np.radians(la)) ** 2
    sun = 6 * np.cos(np.radians(lo + 15 * hour)) * np.cos(np.radians(la))
    return base + sun + rng.normal(0, 0.8, la.shape)


def cf_coords(ds, lat, lon):
    ds.createDimension("latitude", lat.size)
    ds.createDimension("longitude", lon.size)
    v = ds.createVariable("latitude", "f4", ("latitude",))
    v[:] = lat
    v.units, v.standard_name, v.long_name = "degrees_north", "latitude", "latitude"
    v = ds.createVariable("longitude", "f4", ("longitude",))
    v[:] = lon
    v.units, v.standard_name, v.long_name = "degrees_east", "longitude", "longitude"


lat = np.linspace(90, -90, 37, dtype="f4")
lon = np.arange(0, 360, 5.0, dtype="f4")
land = land_mask(lat, lon)
hours = [0, 12]
with netCDF4.Dataset(out / "era5_like.nc", "w", format="NETCDF4") as ds:
    ds.Conventions = "CF-1.6"
    ds.history = "synthetic test data for vw (not real observations)"
    cf_coords(ds, lat, lon)
    ds.createDimension("time", len(hours))
    t = ds.createVariable("time", "i4", ("time",))
    t[:] = np.array(hours) + 1_087_368  # 2024-01-18 の 0 時と 12 時（1900-01-01 からの時間）
    t.units, t.calendar, t.long_name = "hours since 1900-01-01 00:00:00.0", "gregorian", "time"
    t2m = ds.createVariable("t2m", "i2", ("time", "latitude", "longitude"), zlib=True, shuffle=True,
                            complevel=4, chunksizes=(1, 37, 72), fill_value=np.int16(-32767))
    t2m.scale_factor, t2m.add_offset = np.float64(0.0016), np.float64(270.0)
    t2m.units, t2m.long_name, t2m.standard_name = "K", "2 metre temperature", "air_temperature"
    tp = ds.createVariable("tp", "f4", ("time", "latitude", "longitude"), zlib=True, shuffle=True)
    tp.units, tp.long_name = "m", "Total precipitation"
    for k, h in enumerate(hours):
        field = temperature(lat, lon, h)
        field = np.ma.masked_where((~land) & (np.abs(lat)[:, None] > 70) & (lon[None, :] > 300), field)
        t2m[k] = field
        rain = np.clip(rng.gamma(0.4, 0.002, land.shape) * (1 + np.cos(np.radians(lat))[:, None]), 0, None)
        tp[k] = rain.astype("f4")

for name, fmt in [("classic.nc", "NETCDF3_CLASSIC"), ("classic_64bit.nc", "NETCDF3_64BIT_OFFSET")]:
    with netCDF4.Dataset(out / name, "w", format=fmt) as ds:
        ds.Conventions = "CF-1.6"
        ds.title = f"synthetic {fmt} file for vw"
        cf_coords(ds, lat, lon)
        v = ds.createVariable("t2m", "f4", ("latitude", "longitude"), fill_value=np.float32(-9999))
        v.units, v.long_name = "K", "2 metre temperature"
        v[:] = temperature(lat, lon, 12).astype("f4")
        m = ds.createVariable("land_sea_mask", "i1", ("latitude", "longitude"))
        m.long_name, m.flag_values, m.flag_meanings = "land-sea mask", np.array([0, 1], "i1"), "sea land"
        m[:] = land.astype("i1")

slat = np.arange(-87.5, 90, 5.0, dtype="f4")
slon = np.arange(-177.5, 180, 5.0, dtype="f4")
sland = land_mask(slat, slon)
with netCDF4.Dataset(out / "sst_groups.nc", "w", format="NETCDF4") as ds:
    ds.Conventions = "CF-1.8"
    ds.title = "synthetic sea surface temperature with groups"
    cf_coords(ds, slat, slon)
    ds.createDimension("time", None)
    t = ds.createVariable("time", "f8", ("time",))
    t.units = "days since 1981-01-01"
    sst_g = ds.createGroup("sea_surface")
    sst = sst_g.createVariable("analysed_sst", "f4", ("time", "latitude", "longitude"), zlib=True,
                               fill_value=np.float32(-1.0e9))
    sst.units, sst.standard_name = "kelvin", "sea_surface_temperature"
    qual_g = ds.createGroup("quality")
    q = qual_g.createVariable("quality_level", "u1", ("time", "latitude", "longitude"), zlib=True)
    q.flag_values = np.array([0, 1, 2, 3, 4, 5], "u1")
    q.flag_meanings = "no_data bad_data worst_quality low_quality acceptable_quality best_quality"
    ice = sst_g.createVariable("sea_ice_fraction", "i1", ("time", "latitude", "longitude"), zlib=True,
                               fill_value=np.int8(-128))
    ice.set_auto_maskandscale(False)
    ice._Unsigned = "true"
    ice.scale_factor, ice.units = np.float32(0.01), "1"
    for k in range(2):
        t[k] = 15700 + k
        field = 300 - 30 * np.sin(np.radians(slat))[:, None] ** 2 + rng.normal(0, 0.3, sland.shape)
        sst[k] = np.ma.masked_where(sland, field.astype("f4"))
        ql = np.where(sland, 0, rng.integers(3, 6, sland.shape))
        q[k] = ql.astype("u1")
        frac = np.clip((np.abs(slat)[:, None] - 60) / 20, 0, 1) * (~sland)
        ice[k] = np.round(frac * 100).astype("u1").view("i1")

# 境目の書き方。値は位置から決まる簡単なものにして、テストで数えやすくする。
elat = np.arange(-60, 61, 30.0, dtype="f4")  # 南から北へ 5 行
elon = np.arange(0, 360, 90.0, dtype="f4")  # 4 列
with netCDF4.Dataset(out / "edge_cases.nc", "w", format="NETCDF3_CLASSIC") as ds:
    ds.title = "edge cases for vw"
    cf_coords(ds, elat, elon)
    rows, cols = np.meshgrid(np.arange(elat.size), np.arange(elon.size), indexing="ij")
    v = ds.createVariable("south_up", "f4", ("latitude", "longitude"))
    v.units = "1"
    v[:] = (rows * 10 + cols).astype("f4")  # 左下（南の端、経度 0）が 0
    v = ds.createVariable("lon_lat", "f4", ("longitude", "latitude"))
    v.units = "1"
    v[:] = (rows * 10 + cols).astype("f4").T
    # 計測の値（float の scale_factor と同じ型）の範囲。計測で 0〜10 の外を欠損にする。
    v = ds.createVariable("measured_range", "i2", ("latitude", "longitude"))
    v.set_auto_maskandscale(False)
    v.scale_factor, v.add_offset = np.float32(0.5), np.float32(0.0)
    # 属性の代入（v.valid_range = ...）は変数の型に直して書くので、setncattr で float のまま書く。
    v.setncattr("valid_range", np.array([0.0, 10.0], "f4"))
    v[:] = (rows * 6 + cols).astype("i2")  # 計測の値は 0〜13.5
    # 格納値（変数と同じ int16）の範囲。格納値で 0〜10 の外を欠損にする。
    v = ds.createVariable("stored_range", "i2", ("latitude", "longitude"))
    v.set_auto_maskandscale(False)
    v.scale_factor, v.add_offset = np.float32(0.5), np.float32(0.0)
    v.valid_range = np.array([0, 10], "i2")
    v[:] = (rows * 6 + cols).astype("i2")

# 座標の読み方。値は小さく、期待値は geo-expected.txt に書く。
from pyproj import CRS, Transformer  # noqa: E402

geo_lines = []
with netCDF4.Dataset(out / "geo_cases.nc", "w", format="NETCDF4") as ds:
    # 経度 0〜360° の格子。値は列の番号。
    ds.createDimension("lat", 3)
    ds.createDimension("lon", 8)
    v = ds.createVariable("lat", "f8", ("lat",))
    v.units, v.standard_name = "degrees_north", "latitude"
    v[:] = [30.0, 0.0, -30.0]
    v = ds.createVariable("lon", "f8", ("lon",))
    v.units, v.standard_name = "degrees_east", "longitude"
    v[:] = np.arange(8) * 45.0
    v = ds.createVariable("lon360", "f4", ("lat", "lon"))
    v[:] = np.tile(np.arange(8, dtype="f4"), (3, 1))
    # 偏差。valid_range が 0 をはさんで対称で、standard_name にも anomaly。
    v = ds.createVariable("t_anomaly", "f4", ("lat", "lon"))
    v.standard_name = "air_temperature_anomaly"
    v.units = "K"
    v.setncattr("valid_range", np.array([-20.0, 20.0], "f4"))
    v[:] = np.linspace(-5, 15, 24, dtype="f4").reshape(3, 8)

    # 曲がった格子。(y, x) の画素ごとの緯度・経度。
    ds.createDimension("y", 3)
    ds.createDimension("x", 4)
    lat2 = ds.createVariable("lat2d", "f4", ("y", "x"))
    lat2.units, lat2.standard_name = "degrees_north", "latitude"
    lon2 = ds.createVariable("lon2d", "f4", ("y", "x"))
    lon2.units, lon2.standard_name = "degrees_east", "longitude"
    yy, xx = np.meshgrid(np.arange(3), np.arange(4), indexing="ij")
    lat2[:] = 40.0 - yy * 2.0 + xx * 0.5
    lon2[:] = 130.0 + xx * 3.0 + yy * 0.25
    v = ds.createVariable("curvilinear", "f4", ("y", "x"))
    v.coordinates = "lat2d lon2d"
    v[:] = (yy * 4 + xx).astype("f4")
    geo_lines.append(f"curvilinear 2,1 {lat2[2, 1]:.6f} {lon2[2, 1]:.6f}")

    # ランベルト正角円錐図法。x・y は km、北が上（y は北から南へ並べる）。
    ds.createDimension("yc", 3)
    ds.createDimension("xc", 4)
    yc = ds.createVariable("yc", "f8", ("yc",))
    yc.units, yc.standard_name = "km", "projection_y_coordinate"
    yc[:] = [500.0, 0.0, -500.0]
    xc = ds.createVariable("xc", "f8", ("xc",))
    xc.units, xc.standard_name = "km", "projection_x_coordinate"
    xc[:] = [-600.0, -200.0, 200.0, 600.0]
    crs = ds.createVariable("lambert", "i4")
    crs.grid_mapping_name = "lambert_conformal_conic"
    crs.standard_parallel = np.array([30.0, 60.0])
    crs.latitude_of_projection_origin = 35.0
    crs.longitude_of_central_meridian = 135.0
    crs.semi_major_axis = 6378137.0
    crs.inverse_flattening = 298.257223563
    v = ds.createVariable("lcc_field", "f4", ("yc", "xc"))
    v.grid_mapping = "lambert"
    v[:] = np.arange(12, dtype="f4").reshape(3, 4)
    t = Transformer.from_crs(CRS.from_proj4("+proj=lcc +lat_1=30 +lat_2=60 +lat_0=35 +lon_0=135 +ellps=WGS84"),
                             "EPSG:4326", always_xy=True)
    for (r, c) in [(0, 0), (1, 2), (2, 3)]:
        lon, lat = t.transform(xc[c] * 1000.0, yc[r] * 1000.0)
        geo_lines.append(f"lcc_field {r},{c} {lat:.6f} {lon:.6f}")

    # 回転極の格子（北極を 39.25°N, 162°W に置いた格子）。
    ds.createDimension("rlat", 2)
    ds.createDimension("rlon", 3)
    rl = ds.createVariable("rlat", "f8", ("rlat",))
    rl.units, rl.standard_name = "degrees", "grid_latitude"
    rl[:] = [5.0, -5.0]
    ro = ds.createVariable("rlon", "f8", ("rlon",))
    ro.units, ro.standard_name = "degrees", "grid_longitude"
    ro[:] = [-10.0, 0.0, 10.0]
    rp = ds.createVariable("rotated_pole", "i4")
    rp.grid_mapping_name = "rotated_latitude_longitude"
    rp.grid_north_pole_latitude = 39.25
    rp.grid_north_pole_longitude = -162.0
    v = ds.createVariable("rotated_field", "f4", ("rlat", "rlon"))
    v.grid_mapping = "rotated_pole"
    v[:] = np.arange(6, dtype="f4").reshape(2, 3)
    t = Transformer.from_crs(CRS.from_proj4("+proj=ob_tran +o_proj=longlat +o_lat_p=39.25 +o_lon_p=0 +lon_0=18 "
                                            "+R=6371229 +no_defs"),
                             CRS.from_proj4("+proj=longlat +R=6371229 +no_defs"), always_xy=True)
    for (r, c) in [(0, 0), (1, 2)]:
        lon, lat = t.transform(ro[c], rl[r])
        geo_lines.append(f"rotated_field {r},{c} {lat:.6f} {lon:.6f}")
(out / "geo-expected.txt").write_text("\n".join(geo_lines) + "\n")
print("\n".join(geo_lines))

lines = []
for name, var in [("era5_like.nc", "t2m"), ("era5_like.nc", "tp"), ("classic.nc", "t2m"),
                  ("classic_64bit.nc", "land_sea_mask"), ("sst_groups.nc", "sea_surface/analysed_sst"),
                  ("sst_groups.nc", "sea_surface/sea_ice_fraction"), ("sst_groups.nc", "quality/quality_level"),
                  ("edge_cases.nc", "south_up"), ("edge_cases.nc", "lon_lat"),
                  ("edge_cases.nc", "measured_range"), ("edge_cases.nc", "stored_range")]:
    with netCDF4.Dataset(out / name) as ds:
        v = ds[var]
        a = np.ma.filled(v[:].astype("f8"), np.nan)
        if var == "measured_range":
            # netCDF4（Python）は valid_range をいつも格納値と比べる。vw は netCDF-Java と同じく、
            # scale_factor と同じ型の valid_range を計測の値と比べるので、期待値はそれで出す。
            v.set_auto_maskandscale(False)
            a = v[:].astype("f8") * float(v.scale_factor) + float(v.add_offset)
            lo, hi = (float(x) for x in v.valid_range)
            a[(a < lo) | (a > hi)] = np.nan
        first = a.reshape((-1,) + a.shape[-2:])[0]
        lines.append(f"{name} {var} shape={'x'.join(map(str, a.shape))} nan={int(np.isnan(a).sum())} "
                     f"min={np.nanmin(a):.4f} max={np.nanmax(a):.4f} mean={np.nanmean(a):.4f} "
                     f"first={first[0, 0]:.4f} last={first[-1, -1]:.4f}")
(out / "expected-values.txt").write_text("\n".join(lines) + "\n")
print("\n".join(lines))
