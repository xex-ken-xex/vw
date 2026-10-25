//! 画素の変換（ピクセルパイプライン）の計算。ビューア（apps/vw）と、まとめてかけるツール
//! （apps/vwbatch）が同じ結果を出すように、画面の部品に依らない部分をここに置く。

pub mod astro;
pub mod expr;
pub mod look;
pub mod lut;
pub mod nodes;
pub mod pixel;
pub mod preset;
pub mod render;
