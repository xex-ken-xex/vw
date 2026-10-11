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
    pub metadata: Metadata,
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
            metadata: Metadata::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
