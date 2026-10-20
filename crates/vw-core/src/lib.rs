//! vw の中核ライブラリ。
//!
//! 画像は次の 3 段階を通って画面に出る。
//!
//! ```text
//! ファイルのバイト列 --[DecoderPlugin]--> VwImage --[display::to_display]--> DisplayImage (RGBA8)
//! ```
//!
//! - [`VwImage`]: 形式に依存しない vw 共通のメモリ形式。16bit などの階調を落とさずに保持する
//! - [`DecoderPlugin`]: 形式ごとのデコーダ。[`Registry`] に登録して使う
//! - [`DisplayImage`]: GPU に転送する描画用メモリ
//!
//! このクレートは GUI ライブラリに依存しない。
//! GUI を差し替えたり、コマンドライン変換ツールから使ったりできるようにするため。

pub mod display;
pub mod exif;
pub mod image;
pub mod plugin;

pub use display::{DisplayImage, DisplayParams};
pub use image::{ColorModel, Curve, Metadata, PixelData, VwImage};
pub use plugin::{DecodeError, DecoderPlugin, ProbeExtensions, Registry};
