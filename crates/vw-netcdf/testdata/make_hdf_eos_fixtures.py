"""vw-netcdf のテストに使う、HDF-EOS の構造の文字（StructMetadata.0）を持つ小さいファイルを作る。

`python3 make_hdf_eos_fixtures.py [出力フォルダ]`（既定はこのスクリプトのあるフォルダ）。
pyhdf・h5py・numpy・pyproj が要る。値は式で決まる作り物。

hdf_eos2_cases.hdf（HDF4、HDF-EOS2 に似せる。次元の名前は `名前:持ち主`）:
- swath「Swath5km」: Temperature float32 12 × 10（Cell_Along_Swath_1km × Cell_Across_Swath_1km）。値 行 + 列 / 100
  Latitude・Longitude float32 3 × 2（5 km の次元）。DimensionMap は Offset 2・Increment 5。
  緯度 40 + 0.5 行 − 0.2 列、経度 179.5 + 列（180° を越える所は −180 から数える）
- grid「Tile」: 正弦図法（GCTP_SNSOID、球 6371007.181 m）。NDVI int16 4 × 6（YDim × XDim）。
  左上 (−1000000, 4000000) m、右下 (−400000, 3600000) m、画素の中心
- grid「Global」: 緯度経度の格子（GCTP_GEO、度・分・秒を詰めた値）。Cover uint8 4 × 8。全球

hdf_eos5_cases.h5（HDF5、HDF-EOS5 に似せる。次元の名前はない）:
- HDFEOS/GRIDS/Polar/Data Fields/Ice: uint8 5 × 5。ランベルト正積方位図法（HE5_GCTP_LAMAZ、北極が中心、
  球 6371228 m）。左上 (−500000, 500000) m、右下 (500000, −500000) m
- HDFEOS/SWATHS/S/Data Fields/Radiance: float32 10 × 10。Geolocation Fields の Latitude・Longitude は 2 × 2、
  DimensionMap は Offset 0・Increment 9。緯度 10 + 10 行、経度 20 + 10 列。
  データの次元の名前は構造の文字の DataField の DimList にだけある
- HDFEOS INFORMATION/StructMetadata.0: 長さ 32000 の固定長の文字

最後に、テストで確かめる画素の緯度・経度を pyproj で出して表示する（テストの期待値に写す）。
"""

import sys
from pathlib import Path

import h5py
import numpy as np
from pyhdf.SD import SD, SDC
from pyproj import CRS, Transformer

out = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent
out.mkdir(parents=True, exist_ok=True)

EOS2 = """GROUP=SwathStructure
\tGROUP=SWATH_1
\t\tSwathName="Swath5km"
\t\tGROUP=DimensionMap
\t\t\tOBJECT=DimensionMap_1
\t\t\t\tGeoDimension="Cell_Along_Swath_5km"
\t\t\t\tDataDimension="Cell_Along_Swath_1km"
\t\t\t\tOffset=2
\t\t\t\tIncrement=5
\t\t\tEND_OBJECT=DimensionMap_1
\t\t\tOBJECT=DimensionMap_2
\t\t\t\tGeoDimension="Cell_Across_Swath_5km"
\t\t\t\tDataDimension="Cell_Across_Swath_1km"
\t\t\t\tOffset=2
\t\t\t\tIncrement=5
\t\t\tEND_OBJECT=DimensionMap_2
\t\tEND_GROUP=DimensionMap
\t\tGROUP=GeoField
\t\t\tOBJECT=GeoField_1
\t\t\t\tGeoFieldName="Latitude"
\t\t\t\tDimList=("Cell_Along_Swath_5km","Cell_Across_Swath_5km")
\t\t\tEND_OBJECT=GeoField_1
\t\t\tOBJECT=GeoField_2
\t\t\t\tGeoFieldName="Longitude"
\t\t\t\tDimList=("Cell_Along_Swath_5km","Cell_Across_Swath_5km")
\t\t\tEND_OBJECT=GeoField_2
\t\tEND_GROUP=GeoField
\tEND_GROUP=SWATH_1
END_GROUP=SwathStructure
GROUP=GridStructure
\tGROUP=GRID_1
\t\tGridName="Tile"
\t\tXDim=6
\t\tYDim=4
\t\tUpperLeftPointMtrs=(-1000000.000000,4000000.000000)
\t\tLowerRightMtrs=(-400000.000000,3600000.000000)
\t\tProjection=GCTP_SNSOID
\t\tProjParams=(6371007.181000,0,0,0,0,0,0,0,0,0,0,0,0)
\t\tSphereCode=-1
\t\tGridOrigin=HDFE_GD_UL
\tEND_GROUP=GRID_1
\tGROUP=GRID_2
\t\tGridName="Global"
\t\tXDim=8
\t\tYDim=4
\t\tUpperLeftPointMtrs=(-180000000.000000,90000000.000000)
\t\tLowerRightMtrs=(180000000.000000,-90000000.000000)
\t\tProjection=GCTP_GEO
\t\tSphereCode=12
\t\tGridOrigin=HDFE_GD_UL
\tEND_GROUP=GRID_2
END_GROUP=GridStructure
GROUP=PointStructure
END_GROUP=PointStructure
END
"""

path = out / "hdf_eos2_cases.hdf"
sd = SD(str(path), SDC.WRITE | SDC.CREATE | SDC.TRUNC)
setattr(sd, "StructMetadata.0", EOS2)


def make(name, nt, values, dims):
    v = sd.create(name, nt, values.shape)
    for i, d in enumerate(dims):
        v.dim(i).setname(d)
    v[:] = values
    v.endaccess()


r, c = np.meshgrid(np.arange(12), np.arange(10), indexing="ij")
make("Temperature", SDC.FLOAT32, (r + c / 100).astype("f4"),
     ["Cell_Along_Swath_1km:Swath5km", "Cell_Across_Swath_1km:Swath5km"])
gr, gc = np.meshgrid(np.arange(3), np.arange(2), indexing="ij")
lon = 179.5 + gc
lon = np.where(lon > 180, lon - 360, lon)
make("Latitude", SDC.FLOAT32, (40 + 0.5 * gr - 0.2 * gc).astype("f4"),
     ["Cell_Along_Swath_5km:Swath5km", "Cell_Across_Swath_5km:Swath5km"])
make("Longitude", SDC.FLOAT32, lon.astype("f4"),
     ["Cell_Along_Swath_5km:Swath5km", "Cell_Across_Swath_5km:Swath5km"])
tr, tc = np.meshgrid(np.arange(4), np.arange(6), indexing="ij")
make("NDVI", SDC.INT16, (1000 * tr + tc).astype("i2"), ["YDim:Tile", "XDim:Tile"])
cr, cc = np.meshgrid(np.arange(4), np.arange(8), indexing="ij")
make("Cover", SDC.UINT8, (10 * cr + cc).astype("u1"), ["YDim:Global", "XDim:Global"])
sd.end()
print(path)

EOS5 = """GROUP=SwathStructure
\tGROUP=SWATH_1
\t\tSwathName="S"
\t\tGROUP=DimensionMap
\t\t\tOBJECT=DimensionMap_1
\t\t\t\tGeoDimension="GeoTrack"
\t\t\t\tDataDimension="DataTrack"
\t\t\t\tOffset=0
\t\t\t\tIncrement=9
\t\t\tEND_OBJECT=DimensionMap_1
\t\t\tOBJECT=DimensionMap_2
\t\t\t\tGeoDimension="GeoXTrack"
\t\t\t\tDataDimension="DataXTrack"
\t\t\t\tOffset=0
\t\t\t\tIncrement=9
\t\t\tEND_OBJECT=DimensionMap_2
\t\tEND_GROUP=DimensionMap
\t\tGROUP=GeoField
\t\t\tOBJECT=GeoField_1
\t\t\t\tGeoFieldName="Latitude"
\t\t\t\tDimList=("GeoTrack","GeoXTrack")
\t\t\tEND_OBJECT=GeoField_1
\t\t\tOBJECT=GeoField_2
\t\t\t\tGeoFieldName="Longitude"
\t\t\t\tDimList=("GeoTrack","GeoXTrack")
\t\t\tEND_OBJECT=GeoField_2
\t\tEND_GROUP=GeoField
\t\tGROUP=DataField
\t\t\tOBJECT=DataField_1
\t\t\t\tDataFieldName="Radiance"
\t\t\t\tDimList=("DataTrack","DataXTrack")
\t\t\tEND_OBJECT=DataField_1
\t\tEND_GROUP=DataField
\tEND_GROUP=SWATH_1
END_GROUP=SwathStructure
GROUP=GridStructure
\tGROUP=GRID_1
\t\tGridName="Polar"
\t\tXDim=5
\t\tYDim=5
\t\tUpperLeftPointMtrs=(-500000.000000,500000.000000)
\t\tLowerRightMtrs=(500000.000000,-500000.000000)
\t\tProjection=HE5_GCTP_LAMAZ
\t\tProjParams=(6371228,0,0,0,0,90000000,0,0,0,0,0,0,0)
\t\tSphereCode=-1
\t\tGridOrigin=HE5_HDFE_GD_UL
\tEND_GROUP=GRID_1
END_GROUP=GridStructure
GROUP=PointStructure
END_GROUP=PointStructure
GROUP=ZaStructure
END_GROUP=ZaStructure
END
"""

path5 = out / "hdf_eos5_cases.h5"
with h5py.File(path5, "w") as f:
    pr, pc = np.meshgrid(np.arange(5), np.arange(5), indexing="ij")
    f.create_dataset("HDFEOS/GRIDS/Polar/Data Fields/Ice", data=(10 * pr + pc).astype("u1"))
    sr, sc = np.meshgrid(np.arange(10), np.arange(10), indexing="ij")
    f.create_dataset("HDFEOS/SWATHS/S/Data Fields/Radiance", data=(sr + sc / 100).astype("f4"))
    gr, gc = np.meshgrid(np.arange(2), np.arange(2), indexing="ij")
    geo = f.create_group("HDFEOS/SWATHS/S/Geolocation Fields")
    geo.create_dataset("Latitude", data=(10.0 + 10 * gr).astype("f4"))
    geo.create_dataset("Longitude", data=(20.0 + 10 * gc).astype("f4"))
    text = np.frombuffer(EOS5.encode().ljust(32000, b"\0"), dtype="S32000")[0]
    f.create_dataset("HDFEOS INFORMATION/StructMetadata.0", data=text)
print(path5)


def show(name, crs, xs, ys, cells):
    to = Transformer.from_crs(CRS(crs), CRS(crs).geodetic_crs, always_xy=True)
    for row, col in cells:
        lon, lat = to.transform(xs[col], ys[row])
        print(f"{name} ({col}, {row}): ({lat!r}, {lon!r})")


step = 600000 / 6
show("Tile", "+proj=sinu +R=6371007.181 +no_defs",
     [-1000000 + (i + 0.5) * step for i in range(6)],
     [4000000 - (j + 0.5) * 100000 for j in range(4)], [(0, 0), (3, 5)])
show("Polar", "+proj=laea +lat_0=90 +lon_0=0 +R=6371228 +no_defs",
     [-500000 + (i + 0.5) * 200000 for i in range(5)],
     [500000 - (j + 0.5) * 200000 for j in range(5)], [(0, 0), (2, 2), (4, 1)])
