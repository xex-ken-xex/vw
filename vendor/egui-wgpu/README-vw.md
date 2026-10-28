# egui-wgpu 0.36.2（vw 用の写し）

crates.io の egui-wgpu 0.36.2 をそのまま写し、次だけを変えている。

- `src/lib.rs` の `preferred_framebuffer_format`: アプリが `set_preferred_surface_formats` で頼んだ形式を先に選ぶ。
  上流は 8bit の形式（Rgba8Unorm / Bgra8Unorm）があればいつもそれを選び、設定で変えられない。
  vw は 10bit の表示のときに Rgb10a2Unorm を頼む
- `src/lib.rs`: lint を止める（vw の `cargo clippy -- -D warnings` で落ちないように）
- `rustfmt.toml`: 整形しない（vw の `cargo fmt --check` の対象から外す）

上流で描画先の形式を選べるようになったら、ルートの `Cargo.toml` の `[patch.crates-io]` とこのフォルダを消す。

変えたファイルの先頭には、変えたことを英語で書いてある（Apache License 2.0 の 4(b) が、変えたファイルに
その旨を目立つように書くことを求めているため）。egui-wgpu は MIT OR Apache-2.0 で、写しには crate に
入っていない LICENSE-MIT と LICENSE-APACHE を egui のリポジトリから足した。
