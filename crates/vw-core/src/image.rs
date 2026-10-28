//! vw 共通の画像メモリ形式 [`VwImage`] とその構成要素。

/// 1 画素あたりのチャンネル構成。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorModel {
    Gray,
    GrayAlpha,
    Rgb,
    Rgba,
}

impl ColorModel {
    /// 1 画素あたりのサンプル数。
    pub fn channels(self) -> usize {
        match self {
            ColorModel::Gray => 1,
            ColorModel::GrayAlpha => 2,
            ColorModel::Rgb => 3,
            ColorModel::Rgba => 4,
        }
    }
}

/// 画素データ。チャンネルはインターリーブ、行優先、行末のパディングなし。
///
/// - `U8`: 一般的な 8bit 画像（BMP, JPEG, 8bit PNG）
/// - `U16`: 16bit PNG、DICOM、カメラ RAW
/// - `F32`: デコーダ側で実数値にしたもの（DICOM の Rescale Slope/Intercept 適用後など）
///
/// 型ごとに `Vec` を持つ。すべて `f32` に揃える方式は、8bit 画像でメモリが 4 倍になり
/// 変換時間も増えるので採らない。
#[derive(Clone, Debug)]
pub enum PixelData {
    U8(Vec<u8>),
    U16(Vec<u16>),
    F32(Vec<f32>),
}

impl PixelData {
    /// サンプル数（画素数 × チャンネル数）。
    pub fn len(&self) -> usize {
        match self {
            PixelData::U8(v) => v.len(),
            PixelData::U16(v) => v.len(),
            PixelData::F32(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 1 サンプルの格納ビット数。
    pub fn bits_per_sample(&self) -> u32 {
        match self {
            PixelData::U8(_) => 8,
            PixelData::U16(_) => 16,
            PixelData::F32(_) => 32,
        }
    }
}

/// 画像のメタデータ。(グループ, キー, 値) を出現順に保持する。
///
/// グループは "PNG"、"EXIF"、"DICOM" など、表示時の見出しになる。
/// `HashMap` にしないのは、ファイル内の順序をそのまま表示したいのと、
/// テキストチャンクのように同じキーが複数回現れる形式があるため。
#[derive(Clone, Debug, Default)]
pub struct Metadata {
    pub entries: Vec<(String, String, String)>,
}

impl Metadata {
    pub fn push(&mut self, group: impl Into<String>, key: impl Into<String>, value: impl ToString) {
        self.entries
            .push((group.into(), key.into(), value.to_string()));
    }

    /// 最初に見つかった `key` の値。
    pub fn get(&self, group: &str, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(g, k, _)| g == group && k == key)
            .map(|(_, _, v)| v.as_str())
    }
}

/// 黒（0）〜白（1）の間の値 `t` を、表示の明るさ 0〜1 に写す曲線。
///
/// 強さは astropy や SAOImage ds9 の既定に合わせて固定にしている。設定で変えられるように
/// しないのは、黒と白の割合を変えれば見え方の調整はおおむね足りるため。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Curve {
    #[default]
    Linear,
    Sqrt,
    Log,
    Asinh,
}

impl Curve {
    const LOG_A: f32 = 1000.0;
    const ASINH_A: f32 = 0.1;

    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Curve::Linear => t,
            Curve::Sqrt => t.sqrt(),
            Curve::Log => (Self::LOG_A * t).ln_1p() / Self::LOG_A.ln_1p(),
            Curve::Asinh => (t / Self::ASINH_A).asinh() / (1.0 / Self::ASINH_A).asinh(),
        }
    }
}

/// 形式に依存しない、vw 共通の画像メモリ形式。
#[derive(Clone, Debug)]
pub struct VwImage {
    pub width: u32,
    pub height: u32,
    pub color: ColorModel,
    pub data: PixelData,
    /// `U16` サンプルのうち意味のあるビット数（DICOM なら 12、RAW なら 14 など）。
    /// 表示時の階調の上限に使う。全ビットを使う場合は `data.bits_per_sample()` と同じ。
    pub significant_bits: u32,
    /// ファイルが勧める表示の階調 `(黒, 白)`（DICOM のウィンドウ幅/レベルなど）。
    /// 表示時に [`crate::DisplayParams::window`] の指定がなければこれを使う。
    pub window: Option<(f32, f32)>,
    /// RGB の画像で、色ごとに別の `(黒, 白)` を当てるとき（灰色の画像を色に割り当てて
    /// 重ねた合成など）。あれば [`VwImage::window`] の代わりに使う。
    pub channel_windows: Option<[(f32, f32); 3]>,
    /// 黒〜白の間の明るさの付け方。天体の画像で淡い部分を持ち上げるのに使う。
    pub curve: Curve,
    pub metadata: Metadata,
    /// [`crate::DecoderPlugin::develop`] で、この画像の代わりに作れる画像の大きさ `(幅, 高さ)`。
    /// カメラ RAW の埋め込みプレビューを表示したときの、センサーの大きさ（向きは補正後）。
    pub develop_size: Option<(u32, u32)>,
    /// 灰色の実数の画像で、欠損（NaN）を描く色。`None` は黒。色の表を引いても変わらないよう、
    /// 灰色でない（赤・緑・青が揃わない）色にする。
    pub missing: Option<[u8; 3]>,
}

impl VwImage {
    /// バッファの長さが `width × height × チャンネル数` と一致しない場合はエラー。
    pub fn new(
        width: u32,
        height: u32,
        color: ColorModel,
        data: PixelData,
    ) -> Result<Self, String> {
        let expected = width as usize * height as usize * color.channels();
        if data.len() != expected {
            return Err(format!(
                "pixel buffer has {} samples, expected {}x{}x{} = {}",
                data.len(),
                width,
                height,
                color.channels(),
                expected
            ));
        }
        let significant_bits = data.bits_per_sample();
        Ok(Self {
            width,
            height,
            color,
            data,
            significant_bits,
            window: None,
            channel_windows: None,
            curve: Curve::Linear,
            metadata: Metadata::default(),
            develop_size: None,
            missing: None,
        })
    }
}

impl VwImage {
    /// EXIF の向き（Orientation、1〜8）に従って画素を並べ替え、正しい向きにする。
    /// 1 と範囲外の値は何もしない。
    ///
    /// 表示側で回転させずに画素を並べ替えるのは、縮小画像・メタデータの幅と高さ・
    /// ズームの計算をすべて見た目の向きで扱えるようにするため。
    pub fn apply_orientation(&mut self, orientation: u32) {
        if !(2..=8).contains(&orientation) {
            return;
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let channels = self.color.channels();
        self.data = match &self.data {
            PixelData::U8(v) => PixelData::U8(reorient(v, w, h, channels, orientation)),
            PixelData::U16(v) => PixelData::U16(reorient(v, w, h, channels, orientation)),
            PixelData::F32(v) => PixelData::F32(reorient(v, w, h, channels, orientation)),
        };
        if orientation >= 5 {
            std::mem::swap(&mut self.width, &mut self.height);
        }
    }
}

/// 幅 `w`、高さ `h` の画素 `src` を、EXIF の向き `orientation` に従って並べ替えたもの。
fn reorient<T: Copy>(src: &[T], w: usize, h: usize, channels: usize, orientation: u32) -> Vec<T> {
    let (dw, dh) = if orientation >= 5 { (h, w) } else { (w, h) };
    let mut out = Vec::with_capacity(src.len());
    for dy in 0..dh {
        for dx in 0..dw {
            let (sx, sy) = match orientation {
                2 => (w - 1 - dx, dy),
                3 => (w - 1 - dx, h - 1 - dy),
                4 => (dx, h - 1 - dy),
                5 => (dy, dx),
                6 => (dy, h - 1 - dx),
                7 => (w - 1 - dy, h - 1 - dx),
                _ => (w - 1 - dy, dx),
            };
            let at = (sy * w + sx) * channels;
            out.extend_from_slice(&src[at..at + channels]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3x2 のグレー画像。画素の値は左上から 1, 2, 3 / 4, 5, 6。
    fn sample() -> VwImage {
        VwImage::new(
            3,
            2,
            ColorModel::Gray,
            PixelData::U8(vec![1, 2, 3, 4, 5, 6]),
        )
        .unwrap()
    }

    fn oriented(orientation: u32) -> (u32, u32, Vec<u8>) {
        let mut img = sample();
        img.apply_orientation(orientation);
        let PixelData::U8(v) = img.data else {
            unreachable!()
        };
        (img.width, img.height, v)
    }

    #[test]
    fn orientation_1_and_out_of_range_values_leave_pixels_as_they_are() {
        assert_eq!(oriented(1), (3, 2, vec![1, 2, 3, 4, 5, 6]));
        assert_eq!(oriented(0), (3, 2, vec![1, 2, 3, 4, 5, 6]));
        assert_eq!(oriented(9), (3, 2, vec![1, 2, 3, 4, 5, 6]));
    }

    #[test]
    fn orientations_2_to_4_mirror_or_turn_upside_down() {
        assert_eq!(oriented(2), (3, 2, vec![3, 2, 1, 6, 5, 4]));
        assert_eq!(oriented(3), (3, 2, vec![6, 5, 4, 3, 2, 1]));
        assert_eq!(oriented(4), (3, 2, vec![4, 5, 6, 1, 2, 3]));
    }

    #[test]
    fn orientations_5_to_8_turn_the_image_on_its_side() {
        assert_eq!(oriented(5), (2, 3, vec![1, 4, 2, 5, 3, 6]));
        assert_eq!(oriented(6), (2, 3, vec![4, 1, 5, 2, 6, 3]));
        assert_eq!(oriented(7), (2, 3, vec![6, 3, 5, 2, 4, 1]));
        assert_eq!(oriented(8), (2, 3, vec![3, 6, 2, 5, 1, 4]));
    }

    #[test]
    fn orientation_keeps_channels_of_a_pixel_together() {
        let mut img = VwImage::new(
            2,
            1,
            ColorModel::Rgb,
            PixelData::U16(vec![1, 2, 3, 4, 5, 6]),
        )
        .unwrap();
        img.apply_orientation(6);
        assert_eq!((img.width, img.height), (1, 2));
        assert!(matches!(img.data, PixelData::U16(v) if v == [1, 2, 3, 4, 5, 6]));
    }

    #[test]
    fn channels_match_color_model() {
        assert_eq!(ColorModel::Gray.channels(), 1);
        assert_eq!(ColorModel::GrayAlpha.channels(), 2);
        assert_eq!(ColorModel::Rgb.channels(), 3);
        assert_eq!(ColorModel::Rgba.channels(), 4);
    }

    #[test]
    fn new_accepts_buffer_of_exact_size() {
        let img = VwImage::new(3, 2, ColorModel::Rgb, PixelData::U8(vec![0; 18])).unwrap();
        assert_eq!((img.width, img.height), (3, 2));
    }

    #[test]
    fn new_rejects_buffer_of_wrong_size() {
        let err = VwImage::new(3, 2, ColorModel::Rgb, PixelData::U8(vec![0; 17])).unwrap_err();
        assert!(err.contains("17"), "{err}");
    }

    #[test]
    fn significant_bits_default_to_storage_bits() {
        let u8_img = VwImage::new(1, 1, ColorModel::Gray, PixelData::U8(vec![0])).unwrap();
        let u16_img = VwImage::new(1, 1, ColorModel::Gray, PixelData::U16(vec![0])).unwrap();
        let f32_img = VwImage::new(1, 1, ColorModel::Gray, PixelData::F32(vec![0.0])).unwrap();
        assert_eq!(u8_img.significant_bits, 8);
        assert_eq!(u16_img.significant_bits, 16);
        assert_eq!(f32_img.significant_bits, 32);
    }

    #[test]
    fn metadata_keeps_insertion_order_and_duplicates() {
        let mut m = Metadata::default();
        m.push("Text", "Comment", "first");
        m.push("Text", "Comment", "second");
        m.push("PNG", "Bit depth", 8);
        let keys: Vec<_> = m
            .entries
            .iter()
            .map(|(_, k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            keys,
            [
                ("Comment", "first"),
                ("Comment", "second"),
                ("Bit depth", "8")
            ]
        );
        assert_eq!(m.get("Text", "Comment"), Some("first"));
        assert_eq!(m.get("PNG", "Missing"), None);
    }
}
