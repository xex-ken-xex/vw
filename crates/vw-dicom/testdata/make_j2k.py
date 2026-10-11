"""テスト用の小さな JPEG 2000 コードストリームを作る。`python3 make_j2k.py`（Pillow が要る）。

Rust 側に JPEG 2000 のエンコーダが無い（デコーダの openjpeg-sys に書き込みの API は
あるが unsafe で長くなる）ので、Pillow で作ったファイルを置いておく。
"""

import struct

from PIL import Image

GRAY16 = [0, 1000, 2000, 3000, 4095, 300, 40, 7]
RGB8 = bytes([255, 0, 0, 0, 255, 0, 0, 0, 255, 10, 20, 30,
              200, 100, 50, 0, 0, 0, 255, 255, 255, 128, 64, 32])

gray = Image.new("I;16", (4, 2))
gray.frombytes(struct.pack("<8H", *GRAY16), "raw", "I;16")
gray.save("gray16.j2k", irreversible=False)
Image.frombytes("RGB", (4, 2), RGB8).save("rgb8.j2k", irreversible=False, mct=1)
