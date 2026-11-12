//! 画素ごとの空の位置（赤経・赤緯）。天体の画像の十字に使う。

use std::fmt::Debug;

/// 画素と空の座標の対応。形式ごとのデコーダ（FITS の WCS など）が作る。
pub trait SkyMap: Debug + Send + Sync {
    /// 表示の向き（上下を返した後）の画素の中心 `(x, y)`（0 始まり、小数も可）の赤経・赤緯（度）。
    /// 投影の外（全天図の四隅など）は `None`。
    fn at(&self, x: f64, y: f64) -> Option<[f64; 2]>;

    /// 画像が銀河座標で書かれているとき、同じ画素の銀河経度・銀河緯度（度）。
    fn galactic(&self, _x: f64, _y: f64) -> Option<[f64; 2]> {
        None
    }

    /// 画素が乗っている部分（モザイクカメラの検出器）の名前と、その部分のファイルの中の画素
    /// （0 始まり）。1 枚の画像なら `None`。
    ///
    /// 画像の別の欄にせずここに置くのは、部分の中の画素が空の座標と同じ写し方（検出器ごとの
    /// 向きと縮め方）で決まり、ここにしかその写し方がないため。
    fn part(&self, _x: f64, _y: f64) -> Option<(String, [f64; 2])> {
        None
    }
}

/// 銀河経度・銀河緯度（度）の文字（`l 184.557000°  b -5.784000°`）。銀河座標は時分秒を使わない。
pub fn format_galactic(l: f64, b: f64) -> String {
    format!("l {:.6}°  b {b:+.6}°", l.rem_euclid(360.0))
}

/// 赤経・赤緯の書き方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkyFormat {
    /// 時分秒・度分秒（`α 05h35m17.30s  δ -05°23′28.0″`）。
    #[default]
    Sexagesimal,
    /// 度の小数（`α 83.822083°  δ -5.391111°`）。
    Degrees,
}

/// 赤経・赤緯（度）の文字。
pub fn format_radec(ra: f64, dec: f64, format: SkyFormat) -> String {
    let ra = ra.rem_euclid(360.0);
    match format {
        SkyFormat::Degrees => format!("α {ra:.6}°  δ {dec:+.6}°"),
        SkyFormat::Sexagesimal => {
            // 赤経は 0.01 秒（0.15″）、赤緯は 0.1″ まで。1 画素が 0.1″ 前後の宇宙望遠鏡の画像でも
            // 隣の画素と見分けられる細かさ。
            let centi = (ra / 15.0 * 360_000.0).round() as u64 % (24 * 360_000);
            let (h, m, s) = (centi / 360_000, centi / 6000 % 60, centi % 6000);
            let sign = if dec < 0.0 { '-' } else { '+' };
            let deci = (dec.abs() * 36_000.0).round() as u64;
            let (d, am, asec) = (deci / 36_000, deci / 600 % 60, deci % 600);
            format!(
                "α {h:02}h{m:02}m{:02}.{:02}s  δ {sign}{d:02}°{am:02}′{:02}.{}″",
                s / 100,
                s % 100,
                asec / 10,
                asec % 10
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sexagesimal_text_gives_hours_and_degrees() {
        // オリオン大星雲の θ1 Ori C（astropy: 05h35m16.46s -05d23m22.8s）。
        let text = format_radec(83.818583, -5.389667, SkyFormat::Sexagesimal);
        assert_eq!(text, "α 05h35m16.46s  δ -05°23′22.8″");
    }

    #[test]
    fn rounding_carries_into_the_next_minute_and_hour() {
        let text = format_radec(359.999_999_9, 89.999_999_9, SkyFormat::Sexagesimal);
        assert_eq!(text, "α 00h00m00.00s  δ +90°00′00.0″");
    }

    #[test]
    fn degrees_text_keeps_six_decimals_and_a_sign() {
        assert_eq!(
            format_radec(-1.5, 2.25, SkyFormat::Degrees),
            "α 358.500000°  δ +2.250000°"
        );
    }
}
