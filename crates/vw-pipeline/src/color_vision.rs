//! 色覚特性の見え方の模擬。図やカラーマップが、色の見え方の違う人にも読み分けられるかを確かめる。
//!
//! 行列は Machado, Oliveira, Fernandes "A Physiologically-based Model for Simulation of Color
//! Vision Deficiency", IEEE TVCG 15(6), 2009, doi:10.1109/TVCG.2009.113 の、程度 1.0
//! （2 色型）の値。著者が「前もって計算した行列をアプリで使う」ためとして公開しているもの。
//! 程度を選べるようにしないのは、論文の行列は程度ごとに別々に求めたもので、間を線形に
//! 混ぜると論文の模擬と食い違うため。

use serde::{Deserialize, Serialize};

use crate::nodes::Float;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Deficiency {
    /// 1 型（P 型）。赤の錐体が働かない。
    Protan,
    /// 2 型（D 型）。緑の錐体が働かない。いちばん多い。
    #[default]
    Deutan,
    /// 3 型（T 型）。青の錐体が働かない。
    Tritan,
    /// 1 色型。明るさだけで見る。
    Achromat,
}

impl Deficiency {
    pub const ALL: [Deficiency; 4] = [
        Deficiency::Protan,
        Deficiency::Deutan,
        Deficiency::Tritan,
        Deficiency::Achromat,
    ];

    /// 見出し（英語。表示時に訳す）。
    pub fn title(self) -> &'static str {
        match self {
            Deficiency::Protan => "Protan (type 1)",
            Deficiency::Deutan => "Deutan (type 2)",
            Deficiency::Tritan => "Tritan (type 3)",
            Deficiency::Achromat => "Achromat (no color)",
        }
    }

    /// 線形の RGB にかける行列。
    fn matrix(self) -> [[f32; 3]; 3] {
        match self {
            Deficiency::Protan => [
                [0.152_286, 1.052_583, -0.204_868],
                [0.114_503, 0.786_281, 0.099_216],
                [-0.003_882, -0.048_116, 1.051_998],
            ],
            Deficiency::Deutan => [
                [0.367_322, 0.860_646, -0.227_968],
                [0.280_085, 0.672_501, 0.047_413],
                [-0.011_820, 0.042_940, 0.968_881],
            ],
            Deficiency::Tritan => [
                [1.255_528, -0.076_749, -0.178_779],
                [-0.078_411, 0.930_809, 0.147_602],
                [0.004_733, 0.691_367, 0.303_900],
            ],
            Deficiency::Achromat => [[0.2126, 0.7152, 0.0722]; 3],
        }
    }
}

/// `img`（sRGB の 0〜1）を、`kind` の見え方に変える。灰色の画像はどの見え方でも同じなので変えない。
pub fn simulate(img: &mut Float, kind: Deficiency) {
    if img.channels != 3 {
        return;
    }
    let m = kind.matrix();
    for px in img.data.as_chunks_mut::<3>().0 {
        let linear = px.map(to_linear);
        for (out, row) in px.iter_mut().zip(m) {
            let v = row[0] * linear[0] + row[1] * linear[1] + row[2] * linear[2];
            *out = to_srgb(v.clamp(0.0, 1.0));
        }
    }
}

/// 行列は光の量に比例する値で求めたものなので、表示の値（sRGB）から戻してからかける。
fn to_linear(v: f32) -> f32 {
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simulated(rgb: [f32; 3], kind: Deficiency) -> [f32; 3] {
        let mut img = Float::new(1, 1, 3, rgb.to_vec());
        simulate(&mut img, kind);
        [img.data[0], img.data[1], img.data[2]]
    }

    #[test]
    fn gray_stays_gray_for_every_kind() {
        for kind in Deficiency::ALL {
            let [r, g, b] = simulated([0.5, 0.5, 0.5], kind);
            assert!((r - 0.5).abs() < 0.01 && (g - 0.5).abs() < 0.01 && (b - 0.5).abs() < 0.01);
        }
    }

    #[test]
    fn red_and_green_look_alike_to_deutans() {
        let red = simulated([0.8, 0.3, 0.2], Deficiency::Deutan);
        let green = simulated([0.5, 0.5, 0.2], Deficiency::Deutan);
        let before = (0.8f32 - 0.5).abs() + (0.3f32 - 0.5).abs();
        let after = (red[0] - green[0]).abs() + (red[1] - green[1]).abs();
        assert!(after < before / 2.0, "{red:?} {green:?}");
    }

    #[test]
    fn achromats_see_only_brightness() {
        let [r, g, b] = simulated([1.0, 0.0, 0.0], Deficiency::Achromat);
        assert_eq!(r, g);
        assert_eq!(g, b);
    }

    #[test]
    fn gray_images_are_left_as_they_are() {
        let mut img = Float::new(2, 1, 1, vec![0.2, 0.9]);
        simulate(&mut img, Deficiency::Protan);
        assert_eq!(img.data, vec![0.2, 0.9]);
    }
}
