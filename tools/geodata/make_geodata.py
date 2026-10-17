#!/usr/bin/env python3
"""GeoNames のダンプから、vw の地名の表（vw-geonames.tsv.gz）を作る。

使い方:
    python3 make_geodata.py <GeoNames のファイルのフォルダ> <出力ファイル>

フォルダには https://download.geonames.org/export/dump/ の次のファイルを置く（zip は展開しておく）:
    cities500.txt, admin1CodesASCII.txt, countryInfo.txt, alternateNamesV2.txt

CI の Geodata ワークフロー（.github/workflows/geodata.yml）が実行し、Release に置く。
vw はそれをダウンロードし、決めておいたハッシュと合うものだけを使う。

出力はタブ区切りの行を gzip にしたもの。1 列目が行の種類:
    C  国コード  英語名  日本語名
    A  国コード.州コード  英語名  日本語名
    P  緯度  経度  国コード  州コード  人口  英語名  日本語名
日本語名がなければ空。地名データは GeoNames（https://www.geonames.org/、CC BY 4.0）で、
vw が使う列だけを抜き出している。
"""

import gzip
import sys
from pathlib import Path

FORMAT = "# vw-geonames 1"


def read_tsv(path):
    with open(path, encoding="utf-8") as f:
        for line in f:
            if line.startswith("#") or not line.strip():
                continue
            yield line.rstrip("\n").split("\t")


def clean(text):
    return text.replace("\t", " ").replace("\n", " ").strip()


def japanese_names(path, wanted):
    """geonameid ごとの日本語名。優先の名前（isPreferredName）があればそれ、なければ最初のもの。
    昔の名前・通称は使わない。"""
    best = {}
    for row in read_tsv(path):
        if len(row) < 8 or row[2] != "ja":
            continue
        gid = row[1]
        if gid not in wanted or row[6] == "1" or row[7] == "1":
            continue
        preferred = row[4] == "1"
        if gid not in best or (preferred and not best[gid][1]):
            best[gid] = (clean(row[3]), preferred)
    return {gid: name for gid, (name, _) in best.items()}


def main():
    src, out = Path(sys.argv[1]), Path(sys.argv[2])
    countries = [
        (r[0], clean(r[4]), r[16]) for r in read_tsv(src / "countryInfo.txt") if len(r) > 16
    ]
    admin1 = [(r[0], clean(r[1]), r[3]) for r in read_tsv(src / "admin1CodesASCII.txt") if len(r) > 3]
    places = []
    for r in read_tsv(src / "cities500.txt"):
        if len(r) < 15:
            continue
        places.append(
            (f"{float(r[4]):.5f}", f"{float(r[5]):.5f}", r[8], r[10], r[14] or "0", clean(r[1]), r[0])
        )
    wanted = {c[2] for c in countries} | {a[2] for a in admin1} | {p[6] for p in places}
    ja = japanese_names(src / "alternateNamesV2.txt", wanted)

    out.parent.mkdir(parents=True, exist_ok=True)
    # mtime を 0 にして、同じ入力からは同じバイト列を作る。
    with open(out, "wb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", mtime=0) as gz:
        lines = [FORMAT]
        lines += [f"C\t{c}\t{en}\t{ja.get(g, '')}" for c, en, g in sorted(countries)]
        lines += [f"A\t{c}\t{en}\t{ja.get(g, '')}" for c, en, g in sorted(admin1)]
        lines += [
            f"P\t{lat}\t{lon}\t{cc}\t{a1}\t{pop}\t{en}\t{ja.get(g, '')}"
            for lat, lon, cc, a1, pop, en, g in sorted(places, key=lambda p: int(p[6]))
        ]
        gz.write(("\n".join(lines) + "\n").encode("utf-8"))
    print(f"{out}: {len(countries)} countries, {len(admin1)} regions, {len(places)} places, "
          f"{sum(1 for p in places if p[6] in ja)} with Japanese names")


if __name__ == "__main__":
    main()
