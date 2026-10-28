# egui-wgpu 0.36.2（vw 用の写し）

crates.io の egui-wgpu 0.36.2 をそのまま写し、次だけを変えている。

- `src/lib.rs` の `preferred_framebuffer_format`: アプリが `set_preferred_surface_formats` で頼んだ形式を先に選ぶ。
  上流は 8bit の形式（Rgba8Unorm / Bgra8Unorm）があればいつもそれを選び、設定で変えられない。
  vw は 10bit の表示のときに Rgb10a2Unorm を、12bit 以上の表示のときに Rgba16Float を頼む
- `src/lib.rs` の描画先の形式の候補: Rgba16Float は、値を線形の scRGB として読む面のときだけ候補に残す。
  そうでない面に 16bit の浮動小数点で描くと、vw と egui が線形の値で描いた絵が白っぽく見えるため
- `src/renderer.rs`: Rgba16Float の面にも、線形の値を書くシェーダー（`fs_main_linear_framebuffer`）を使う。
  上流は sRGB の形式のときだけ線形で書き、Rgba16Float にはガンマのかかった値を書いてしまう
- `src/lib.rs`: lint を止める（vw の `cargo clippy -- -D warnings` で落ちないように）
- `rustfmt.toml`: 整形しない（vw の `cargo fmt --check` の対象から外す）

上流で描画先の形式を選べるようになったら、ルートの `Cargo.toml` の `[patch.crates-io]` とこのフォルダを消す。

変えたファイルの先頭には、変えたことを英語で書いてある（Apache License 2.0 の 4(b) が、変えたファイルに
その旨を目立つように書くことを求めているため）。egui-wgpu は MIT OR Apache-2.0 で、写しには crate に
入っていない LICENSE-MIT と LICENSE-APACHE を egui のリポジトリから足した。
