#!/usr/bin/env python3
"""d3-celestial のデータから、vw の星図の背景（apps/vw/assets/sky/sky.tsv）を作る。

使い方:
    python3 make_skydata.py <d3-celestial の data フォルダ> <出力ファイル>

data フォルダは npm の d3-celestial（BSD-3-Clause、https://github.com/ofrohn/d3-celestial）の
data/ をそのまま使う。読むのは stars.6.json、constellations.json、constellations.lines.json、
constellations.borders.json。

出力はタブ区切りの行。1 列目が行の種類で、座標は赤経・赤緯（度、小数 4 桁）:
    S  赤経  赤緯  等級  B-V          6 等級までの星
    L  星座  赤経,赤緯 赤経,赤緯 …    星座の線（折れ線 1 本）
    B  赤経,赤緯 赤経,赤緯 …          星座の境界（折れ線 1 本）
    N  星座  赤経  赤緯  IAU の名前  日本語名   星座名を置く位置
"""

import json
import sys
from pathlib import Path

FORMAT = "# vw-sky 1"


def ra(lon):
    return f"{lon % 360:.4f}"


def point(p):
    return f"{ra(p[0])},{p[1]:.4f}"


def main():
    data, out = Path(sys.argv[1]), Path(sys.argv[2])
    load = lambda name: json.loads((data / name).read_text(encoding="utf-8"))
    rows = [FORMAT]
    stars = load("stars.6.json")["features"]
    for f in sorted(stars, key=lambda f: float(f["properties"]["mag"])):
        lon, lat = f["geometry"]["coordinates"]
        p = f["properties"]
        bv = p.get("bv") or ""
        rows.append(f"S\t{ra(lon)}\t{lat:.4f}\t{float(p['mag']):.2f}\t{bv}")
    for f in load("constellations.lines.json")["features"]:
        for line in f["geometry"]["coordinates"]:
            rows.append(f"L\t{f['id']}\t" + " ".join(point(p) for p in line))
    for f in load("constellations.borders.json")["features"]:
        for line in f["geometry"]["coordinates"]:
            rows.append("B\t" + " ".join(point(p) for p in line))
    for f in load("constellations.json")["features"]:
        lon, lat = f["geometry"]["coordinates"]
        p = f["properties"]
        rows.append(f"N\t{f['id']}\t{ra(lon)}\t{lat:.4f}\t{p['name']}\t{p.get('ja', '')}")
    out.write_text("\n".join(rows) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
