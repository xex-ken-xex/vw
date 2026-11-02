# vw-netcdf のテスト用データ

`cargo test -p vw-netcdf` が読む、小さい NetCDF のファイルと期待値です。
ファイルはリポジトリに入れてあるので、テストを流すだけなら何も用意しなくてかまいません。
`make_fixtures.py` は、これらを作り直すときだけ使います。

## 中身

| ファイル | 形式 | 確かめること |
| --- | --- | --- |
| `era5_like.nc` | NetCDF-4 | 時刻 2 × 緯度 37 × 経度 72 の気温。int16 を `scale_factor`・`add_offset` で戻す。一部が `_FillValue`。緯度は北から南 |
| `classic.nc` | NetCDF-3（CDF-1） | 気温と、陸と海の印（`flag_values`） |
| `classic_64bit.nc` | NetCDF-3（CDF-2） | `classic.nc` と同じ中身を 64bit オフセットの形で |
| `sst_groups.nc` | NetCDF-4 | グループ、無制限の次元、陸の `_FillValue`、`_Unsigned` の byte |
| `edge_cases.nc` | NetCDF-3 | 緯度が南から北の変数、(経度, 緯度) の順の変数、計測の値の `valid_range` と格納値の `valid_range` |
| `geo_cases.nc` | NetCDF-4 | 緯度・経度の読み方。経度 0〜360° の格子、偏差の変数（`t_anomaly`）、2 次元の緯度・経度の表（曲がった格子）、ランベルト正角円錐図法（x / y が km）、回転極 |
| `hdf5_cases.h5` | HDF5（NetCDF-4 の印なし） | HDF-EOS5 に似せた並び（Data Fields と Geolocation Fields の緯度・経度）、`scale_factor` と `_FillValue`、LZF 圧縮、3 次元、印の変数（`flag_meanings`）、列挙型と構造体（画像にしない）。期待値はテストの中の式 |
| `hdf4_cases.hdf` | HDF4 | SD の API で書いたデータセット。HDF4 の校正（SDsetcal）と `_FillValue`、deflate・RLE・Huffman の圧縮、チャンク（deflate と、端が半端な圧縮なし）、3 次元、Vgroup に入れた緯度・経度、全体の属性。期待値はテストの中の式 |
| `expected-values.txt` | テキスト | 上のファイルを netCDF4 で読んだときの形・欠損の数・最小・最大・平均・角の値 |
| `geo-expected.txt` | テキスト | `geo_cases.nc` の図法と回転極の格子の、画素（行,列）ごとの緯度・経度。pyproj で逆投影した値 |

値はどれも作り物です（乱数の種は固定）。

## make_fixtures.py

### 前提

- Python 3.9 以上
- `netCDF4`・`numpy`・`pyproj`（`pip install netCDF4 numpy pyproj`）。pyproj は `geo-expected.txt` の緯度・経度を、vw とは別の実装で求めるために使います

Windows に Python がなくても、テストを流すのには要りません。作り直すときだけ、Python のある Linux などで実行します。

### できること

- 上の表の `.nc` ファイル 6 つと、`expected-values.txt`・`geo-expected.txt` を作ります。
- `expected-values.txt` の値は netCDF4 で読んだ値です。ただし `edge_cases.nc` の `measured_range` だけは、
  `valid_range` を計測の値（`scale_factor` を当てた後）と比べた値にしています。`valid_range` の型が
  `scale_factor` の型と同じで、変数の型と違うとき、vw はそう読むためです。

### 使い方

```sh
cd crates/vw-netcdf/testdata
python3 make_fixtures.py          # このフォルダに作り直す
python3 make_fixtures.py /tmp/nc  # 別のフォルダに作る
```

作り直したら `cargo test -p vw-netcdf` を流し、期待値と合うことを確かめてください。

## make_hdf5_fixtures.py

### 前提

- Python 3.9 以上
- `h5py`・`numpy`（`pip install h5py numpy`）。LZF の圧縮は h5py に入っています

### できること

- `hdf5_cases.h5` を作ります。値は式で決まるので、期待値のファイルはありません（テストが同じ式で確かめます）。

### 使い方

```sh
cd crates/vw-netcdf/testdata
python3 make_hdf5_fixtures.py          # このフォルダに作り直す
python3 make_hdf5_fixtures.py /tmp/h5  # 別のフォルダに作る
```

## make_hdf4_fixtures.py

### 前提

- Python 3.9 以上
- `pyhdf`・`numpy`（`pip install pyhdf numpy`）。pyhdf の wheel に HDF4 の公式ライブラリが入っています。Linux で確かめました
- チャンクは pyhdf に入口がないので、wheel の中の `libmfhdf` を ctypes で呼びます（`pyhdf.libs` フォルダにあるもの）

### できること

- `hdf4_cases.hdf` を作ります。値は式で決まるので、期待値のファイルはありません（テストが同じ式で確かめます）。
- szip は、pyhdf の入ったライブラリに圧縮する側がないので作りません。

### 使い方

```sh
cd crates/vw-netcdf/testdata
python3 make_hdf4_fixtures.py          # このフォルダに作り直す
python3 make_hdf4_fixtures.py /tmp/h4  # 別のフォルダに作る
```
