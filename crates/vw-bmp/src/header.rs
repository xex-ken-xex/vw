//! BMP のヘッダ（ファイルヘッダと BITMAPINFOHEADER など）。

use vw_core::DecodeError;

pub const BI_RGB: u32 = 0;
pub const BI_RLE4: u32 = 2;
pub const BI_BITFIELDS: u32 = 3;
pub const BI_ALPHABITFIELDS: u32 = 6;

#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    /// 画素データの位置（ファイルの先頭から）。
    pub offset: usize,
    pub info_size: u32,
    pub width: i32,
    /// 負なら上の行から並んでいる。
    pub height: i32,
    pub bits: u16,
    pub compression: u32,
    /// 1 メートルあたりの画素数。
    pub resolution: Option<(u32, u32)>,
    pub colors_used: u32,
}

impl Header {
    pub fn parse(data: &[u8]) -> Result<Self, DecodeError> {
        let r = Reader(data);
        let info_size = r.u32(14)?;
        if info_size == 12 {
            // OS/2 1.x（BITMAPCOREHEADER）は幅・高さが 16bit で、圧縮や解像度の項目がない。
            return Ok(Header {
                offset: r.u32(10)? as usize,
                info_size,
                width: i32::from(r.u16(18)?),
                height: i32::from(r.u16(20)? as i16),
                bits: r.u16(24)?,
                compression: BI_RGB,
                resolution: None,
                colors_used: 0,
            });
        }
        let resolution = (r.u32(38)?, r.u32(42)?);
        Ok(Header {
            offset: r.u32(10)? as usize,
            info_size,
            width: r.u32(18)? as i32,
            height: r.u32(22)? as i32,
            bits: r.u16(28)?,
            compression: r.u32(30)?,
            resolution: (resolution.0 > 0 && resolution.1 > 0).then_some(resolution),
            colors_used: r.u32(46)?,
        })
    }

    pub fn top_down(&self) -> bool {
        self.height < 0
    }

    /// 赤・緑・青の位置を表すビットマスク。BITMAPINFOHEADER なら直後の 12 バイト、
    /// それより新しいヘッダならヘッダの中の同じ位置にある。
    pub fn masks(&self, data: &[u8]) -> Result<[u32; 3], DecodeError> {
        let r = Reader(data);
        Ok([r.u32(54)?, r.u32(58)?, r.u32(62)?])
    }
}

/// リトルエンディアンの数を、ファイルが短いときはエラーにして読む。
pub struct Reader<'a>(pub &'a [u8]);

impl Reader<'_> {
    pub fn u16(&self, at: usize) -> Result<u16, DecodeError> {
        self.bytes::<2>(at).map(u16::from_le_bytes)
    }

    pub fn u32(&self, at: usize) -> Result<u32, DecodeError> {
        self.bytes::<4>(at).map(u32::from_le_bytes)
    }

    fn bytes<const N: usize>(&self, at: usize) -> Result<[u8; N], DecodeError> {
        self.0
            .get(at..at + N)
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| DecodeError::Invalid("BMP header is cut short".into()))
    }
}
