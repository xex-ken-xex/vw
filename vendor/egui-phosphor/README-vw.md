# egui-phosphor 0.14.0（vw 用の写し）

crates.io の egui-phosphor 0.14.0 をそのまま写し、次だけを変えている。

- `Cargo.toml`: egui の版を `0.36` から `0.37` に上げた。上流の 0.14.0 は egui 0.36 までしか受け付けず、
  0.37 向けの版がまだ出ていない。中身はアイコンのフォントと文字の定数だけで、egui の API は使っていない
- `Cargo.toml`: 例・テスト・dev-dependencies を外した（ワークスペースのビルドと配布に要らない）

上流が egui 0.37 を受け付ける版を出したら、ルートの `Cargo.toml` の `[patch.crates-io]` とこのフォルダを消す。

egui-phosphor は MIT OR Apache-2.0 で、LICENSE-MIT と LICENSE-APACHE は crate に入っていた物をそのまま置いている。
Phosphor Icons のフォント（`res/`）は MIT。
