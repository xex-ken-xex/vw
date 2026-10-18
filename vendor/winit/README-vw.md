# winit 0.30.13（vw 用の写し）

crates.io の winit 0.30.13 をそのまま写し、次だけを変えている。

- `src/platform_impl/windows/event_loop.rs` の `WM_DPICHANGED`: ドラッグ中は Windows が勧める矩形をそのまま使う
  （上流の PR https://github.com/rust-windowing/winit/pull/4694 と同じ考え方。issue #4600）
- `src/lib.rs`: lint を止める（vw の `cargo clippy -- -D warnings` で落ちないように）
- `rustfmt.toml`: 整形しない（vw の `cargo fmt --check` の対象から外す）

上流で直った 0.30.x が出たら、ルートの `Cargo.toml` の `[patch.crates-io]` とこのフォルダを消す。

変えたファイルの先頭には、変えたことを英語で書いてある（Apache License 2.0 の 4(b) が、変えたファイルに
その旨を目立つように書くことを求めているため）。
