/// Channel layout of a [`VwImage`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorModel {
    Gray,
    GrayAlpha,
    Rgb,
    Rgba,
}

impl ColorModel {
    pub fn channels(self) -> usize {
        match self {
            ColorModel::Gray => 1,
            ColorModel::GrayAlpha => 2,
            ColorModel::Rgb => 3,
            ColorModel::Rgba => 4,
        }
    }
}

/// Interleaved samples, row-major, no row padding.
///
/// 16-bit covers PNG-16, DICOM and camera RAW; `F32` is for decoders that
/// already produce linear or rescaled values (e.g. DICOM rescale slope/intercept).
#[derive(Clone, Debug)]
pub enum PixelData {
    U8(Vec<u8>),
    U16(Vec<u16>),
    F32(Vec<f32>),
}

impl PixelData {
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

    pub fn bits_per_sample(&self) -> u32 {
        match self {
            PixelData::U8(_) => 8,
            PixelData::U16(_) => 16,
            PixelData::F32(_) => 32,
        }
    }
}

/// Ordered key/value metadata grouped by section (e.g. "PNG", "EXIF", "DICOM").
#[derive(Clone, Debug, Default)]
pub struct Metadata {
    pub entries: Vec<(String, String, String)>,
}

impl Metadata {
    pub fn push(&mut self, group: impl Into<String>, key: impl Into<String>, value: impl ToString) {
        self.entries
            .push((group.into(), key.into(), value.to_string()));
    }
}

/// The format-independent in-memory image vw works with.
#[derive(Clone, Debug)]
pub struct VwImage {
    pub width: u32,
    pub height: u32,
    pub color: ColorModel,
    pub data: PixelData,
    /// Number of significant bits in U16 samples (e.g. 12 for many DICOM / 14 for RAW).
    /// Equal to `data.bits_per_sample()` when every bit is used.
    pub significant_bits: u32,
    pub metadata: Metadata,
}

impl VwImage {
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
            metadata: Metadata::default(),
        })
    }
}
