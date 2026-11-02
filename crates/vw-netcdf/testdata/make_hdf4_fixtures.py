"""vw-netcdf のテストに使う、小さい HDF4 のファイルを作る。

`python3 make_hdf4_fixtures.py [出力フォルダ]`（既定はこのスクリプトのあるフォルダ）。pyhdf・numpy が要る。
szip は pyhdf の入ったライブラリに圧縮する側がないので作らない。値は式で決まる作り物で、テストは同じ式で期待値を出す（期待値のファイルはない）。
チャンクは pyhdf に入口がないので、pyhdf が持つ HDF4 の公式ライブラリの SDsetchunk を ctypes で呼ぶ。

hdf4_cases.hdf（行 r・列 c、4 × 5）:
- calibrated: int16。格納値 20 + 10 r + c、SDsetcal(0.5, 誤差 0, オフセット 10) で値 = 0.5 × (格納値 − 10)。
  _FillValue −999 を左上に
- deflated: float32、deflate。値 r + 0.5 c
- rle: uint8、RLE。値 (r + c) % 3
- skphuff: int32、Huffman（skip 4）。値 1000 r + c
- chunked: uint16 8 × 8、チャンク 4 × 4 を deflate。値 8 r + c
- chunked_plain: int16 8 × 8、チャンク 3 × 3（端が半端）を圧縮なし。値 −(8 r + c)
- cube: float32 3 × 4 × 5。値 100 面 + 5 r + c
- Radiance・Latitude・Longitude: float32。Vgroup「Swath/Data Fields」「Swath/Geolocation Fields」に入れる。
  緯度 30 + r、経度 130 + c
- 全体の属性 title、StructMetadata.0（HDF-EOS2 に似せた文字）、4 × 5 の次元の名前 row・col
"""

import ctypes
import sys
from pathlib import Path

import numpy as np
import pyhdf
import pyhdf.V  # noqa: F401  vgstart が使う
from pyhdf.HDF import HDF, HC
from pyhdf.SD import SD, SDC

out = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent
out.mkdir(parents=True, exist_ok=True)
path = out / "hdf4_cases.hdf"

libs = Path(pyhdf.__file__).resolve().parent.parent / "pyhdf.libs"
mfhdf = ctypes.CDLL(str(next(libs.glob("libmfhdf-*.so*"))))


class ChunkDef(ctypes.Structure):
    # HDF_CHUNK_DEF の comp の形。chunk_lengths[32]、comp_type、model_type、cinfo（deflate は level だけ）。
    _fields_ = [("chunk_lengths", ctypes.c_int32 * 32), ("comp_type", ctypes.c_int32),
                ("model_type", ctypes.c_int32), ("cinfo", ctypes.c_int32 * 8), ("minfo", ctypes.c_int32 * 8)]


def set_chunk(sds, lengths, deflate):
    d = ChunkDef()
    for i, n in enumerate(lengths):
        d.chunk_lengths[i] = n
    flags = 1  # HDF_CHUNK
    if deflate:
        d.comp_type, d.cinfo[0] = SDC.COMP_DEFLATE, 6
        flags |= 2  # HDF_COMP
    assert mfhdf.SDsetchunk(sds._id, d, flags) == 0


r, c = np.meshgrid(np.arange(4), np.arange(5), indexing="ij")
r8, c8 = np.meshgrid(np.arange(8), np.arange(8), indexing="ij")
sd = SD(str(path), SDC.WRITE | SDC.CREATE | SDC.TRUNC)
sd.title = "vw HDF4 test"
setattr(sd, "StructMetadata.0", "GROUP=SwathStructure\n\tGROUP=SWATH_1\n\t\tSwathName=\"Swath\"\n"
        "\tEND_GROUP=SWATH_1\nEND_GROUP=SwathStructure\nEND\n")


def make(name, nt, shape, values, compress=None, chunk=None):
    v = sd.create(name, nt, shape)
    if compress is not None:
        v.setcompress(*compress)
    if chunk is not None:
        set_chunk(v, *chunk)
    if shape == (4, 5):
        v.dim(0).setname("row")
        v.dim(1).setname("col")
    v[:] = values
    return v


v = make("calibrated", SDC.INT16, (4, 5), np.where((r == 0) & (c == 0), -999, 20 + 10 * r + c).astype("i2"))
v.setcal(0.5, 0.0, 10.0, 0.0, SDC.INT16)
v.setfillvalue(-999)
v.units = "K"
v.long_name = "calibrated temperature"
v.endaccess()
make("deflated", SDC.FLOAT32, (4, 5), (r + 0.5 * c).astype("f4"), (SDC.COMP_DEFLATE, 6)).endaccess()
make("rle", SDC.UINT8, (4, 5), ((r + c) % 3).astype("u1"), (SDC.COMP_RLE,)).endaccess()
make("skphuff", SDC.INT32, (4, 5), (1000 * r + c).astype("i4"), (SDC.COMP_SKPHUFF, 4)).endaccess()
make("chunked", SDC.UINT16, (8, 8), (8 * r8 + c8).astype("u2"), chunk=((4, 4), True)).endaccess()
make("chunked_plain", SDC.INT16, (8, 8), (-(8 * r8 + c8)).astype("i2"), chunk=((3, 3), False)).endaccess()
k, r3, c3 = np.meshgrid(np.arange(3), np.arange(4), np.arange(5), indexing="ij")
make("cube", SDC.FLOAT32, (3, 4, 5), (100 * k + 5 * r3 + c3).astype("f4")).endaccess()
refs = {}
for name, values in [("Radiance", (10 + r + c).astype("f4")), ("Latitude", (30.0 + r).astype("f4")),
                     ("Longitude", (130.0 + c).astype("f4"))]:
    v = make(name, SDC.FLOAT32, (4, 5), values)
    refs[name] = v.ref()
    v.endaccess()
sd.end()

# HDF-EOS2 のように、Vgroup にデータセットを入れる。
h = HDF(str(path), HC.WRITE)
vg = h.vgstart()
swath = vg.create("Swath")
swath._class = "SWATH"
for group, names in [("Data Fields", ["Radiance"]), ("Geolocation Fields", ["Latitude", "Longitude"])]:
    g = vg.create(group)
    g._class = "SWATH Vgroup"
    for n in names:
        g.add(HC.DFTAG_NDG, refs[n])
    swath.insert(g)
    g.detach()
swath.detach()
vg.end()
h.close()
print(path)
