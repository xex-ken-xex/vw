//! ファイル全体を読まずに、マルチフレームのフレームを 1 枚ずつ読む。
//!
//! 最初にヘッダ（画素データの手前まで）とフレームの場所の表だけを読み、以後はフレームごとに
//! その場所だけを読む。ネットワーク越しの数 GB のファイルでも、最初のフレームを全体の読み込みを
//! 待たずに出せ、持つのはヘッダと読んだフレームの分だけになる。
//!
//! 1 フレームの画素だけを入れた 1 フレームのオブジェクトを作り、ふつうの読み方（[`decode_one`]）
//! に渡す。dicom-pixeldata にファイルの場所を渡して読ませる口はないので、画素を自前で
//! 取り出してから渡す。

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

use dicom_core::value::{DataSetSequence, PixelFragmentSequence, Value};
use dicom_core::{DataElement, Length, PrimitiveValue, VR};
use dicom_dictionary_std::tags;
use dicom_object::{DefaultDicomObject, InMemDicomObject};
use vw_core::{DecodeError, FrameSource, Metadata, VwImage};

use crate::structure::{self, Located, PixelDataAt};
use crate::{Colors, check_pixel_size, decode_one, frames_in, int, metadata, open};

/// 場所を指して読めるファイル。テストでは読んだバイト数を数える。
pub trait ReadAt: Send + Sync {
    fn size(&self) -> u64;

    /// `at` から `buf` の長さだけ読む。足りなければエラー。
    fn read_at(&self, at: u64, buf: &mut [u8]) -> std::io::Result<()>;
}

/// ディスク（ネットワークドライブも）のファイル。
pub struct FileAt {
    file: Mutex<File>,
    len: u64,
}

impl FileAt {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self {
            file: Mutex::new(file),
            len,
        })
    }
}

impl ReadAt for FileAt {
    fn size(&self) -> u64 {
        self.len
    }

    fn read_at(&self, at: u64, buf: &mut [u8]) -> std::io::Result<()> {
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        file.seek(SeekFrom::Start(at))?;
        file.read_exact(buf)
    }
}

/// 頭を最初に読む大きさ。たいていのヘッダはこれに収まる。
const FIRST_HEAD: u64 = 64 << 10;
/// 頭をここまで読み足しても画素データに届かなければ、場所だけ読むのをあきらめる。
/// Per-frame Functional Groups が大きくても数 MB で、これを超えるのは壊れたファイル。
const MAX_HEAD: u64 = 64 << 20;

/// ファイルの頭。画素データがあれば、その要素のヘッダまで（値は含まない）。
pub struct Head {
    pub bytes: Vec<u8>,
    pub pixel: Option<PixelDataAt>,
}

/// 画素データの要素のヘッダまでを、足りなければ倍ずつ読み足して読む。辿れない形
/// （deflate・ビッグエンディアン）や、頭が [`MAX_HEAD`] を超えるときは `None`。
pub fn read_head(src: &dyn ReadAt) -> Result<Option<Head>, DecodeError> {
    let total = src.size();
    let mut bytes = Vec::new();
    let mut want = FIRST_HEAD.min(total);
    loop {
        let have = bytes.len() as u64;
        bytes.resize(want as usize, 0);
        src.read_at(have, &mut bytes[have as usize..])?;
        match structure::locate_pixel_data(&bytes, total)? {
            Located::At(at) => {
                bytes.truncate(at.value_at as usize);
                return Ok(Some(Head {
                    bytes,
                    pixel: Some(at),
                }));
            }
            Located::Absent => return Ok(Some(Head { bytes, pixel: None })),
            Located::Unwalkable => return Ok(None),
            Located::NeedMore if want >= MAX_HEAD => return Ok(None),
            Located::NeedMore => want = (want * 2).min(total).min(MAX_HEAD),
        }
    }
}

/// ヘッダだけを読む用の頭。辿れないファイルは全体を読む。
pub fn read_head_file(path: &Path) -> Result<Vec<u8>, DecodeError> {
    let src = FileAt::open(path)?;
    match read_head(&src)? {
        Some(head) => Ok(head.bytes),
        None => Ok(std::fs::read(path)?),
    }
}

/// フレームの場所。
enum Layout {
    /// 非圧縮。フレームは `at` から `size` バイトずつ並ぶ。
    Native { at: u64, size: u64, vr: VR },
    /// 圧縮。フレームごとの断片の（位置, 長さ）。
    Fragments(Vec<Vec<(u64, u32)>>),
}

/// フレームを 1 枚ずつ読めるマルチフレームのファイル。
pub struct FileFrames {
    src: Box<dyn ReadAt>,
    /// 画素データと Per-frame Functional Groups を除いた、1 フレームのヘッダ。
    base: DefaultDicomObject,
    per_frame: Vec<InMemDicomObject>,
    colors: Colors,
    meta: Metadata,
    frames: usize,
    layout: Layout,
}

const ITEM: (u16, u16) = (0xFFFE, 0xE000);
const SEQUENCE_END: (u16, u16) = (0xFFFE, 0xE0DD);

impl FileFrames {
    /// ヘッダと場所の表だけを読んで開く。1 枚だけのファイルや、フレームの場所を出せないファイルは
    /// `None`（呼ぶ側がファイル全体を読む）。
    pub fn open(src: Box<dyn ReadAt>) -> Result<Option<Self>, DecodeError> {
        let Some(head) = read_head(&*src)? else {
            return Ok(None);
        };
        let Some(pixel) = head.pixel else {
            return Ok(None);
        };
        let mut obj = open(&head.bytes, true)?;
        let frames = frames_in(&obj);
        if frames < 2 {
            return Ok(None);
        }
        check_pixel_size(&obj)?;
        let colors = Colors::of(&obj)?;
        let layout = match pixel.length {
            Some(length) => native_layout(&obj, &colors, pixel.value_at, length, frames),
            None => fragment_layout(&*src, pixel.value_at, frames)?,
        };
        let Some(layout) = layout else {
            return Ok(None);
        };
        let mut meta = metadata(&obj);
        // 頭は画素データの手前までなので、ファイル全体を読んだときと同じ一覧になるよう足す。
        let listed = match &layout {
            Layout::Native { vr, .. } => format!("({vr:?}, {} bytes)", pixel.length.unwrap_or(0)),
            Layout::Fragments(frames) => {
                format!("({} fragments)", frames.iter().map(Vec::len).sum::<usize>())
            }
        };
        meta.push(crate::TAG_LISTING, "(7FE0,0010) PixelData", listed);
        let per_frame = obj
            .take_element(tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE)
            .ok()
            .and_then(|e| e.into_value().into_items())
            .map(|items| items.into_vec())
            .unwrap_or_default();
        obj.put(DataElement::new(
            tags::NUMBER_OF_FRAMES,
            VR::IS,
            PrimitiveValue::from("1"),
        ));
        Ok(Some(Self {
            src,
            base: obj,
            per_frame,
            colors,
            meta,
            frames,
            layout,
        }))
    }

    /// `frame` 番目のフレームだけを入れた 1 フレームのオブジェクト。
    fn single(&self, frame: usize) -> Result<DefaultDicomObject, DecodeError> {
        let mut obj = self.base.clone();
        if let Some(item) = self.per_frame.get(frame) {
            obj.put(DataElement::new(
                tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE,
                VR::SQ,
                DataSetSequence::new(vec![item.clone()], Length::UNDEFINED),
            ));
        }
        let pixels = match &self.layout {
            Layout::Native { at, size, vr } => {
                let mut bytes = vec![0; *size as usize];
                self.src.read_at(at + size * frame as u64, &mut bytes)?;
                DataElement::new(tags::PIXEL_DATA, *vr, PrimitiveValue::from(bytes))
            }
            Layout::Fragments(frames) => {
                let fragments = frames[frame]
                    .iter()
                    .map(|&(at, len)| {
                        let mut bytes = vec![0; len as usize];
                        self.src.read_at(at, &mut bytes).map(|()| bytes)
                    })
                    .collect::<std::io::Result<Vec<_>>>()?;
                DataElement::new(
                    tags::PIXEL_DATA,
                    VR::OB,
                    Value::PixelSequence(PixelFragmentSequence::new(Vec::<u32>::new(), fragments)),
                )
            }
        };
        obj.put(pixels);
        Ok(obj)
    }
}

impl FrameSource for FileFrames {
    fn frame_count(&self) -> usize {
        self.frames
    }

    fn file_size(&self) -> u64 {
        self.src.size()
    }

    fn decode_frame(&self, frame: usize) -> Result<VwImage, DecodeError> {
        if frame >= self.frames {
            return Err(DecodeError::Invalid(format!(
                "no frame {frame} (the file has {})",
                self.frames
            )));
        }
        let obj = self.single(frame)?;
        let mut img = decode_one(&obj, &self.colors, 0)?;
        img.metadata = self.meta.clone();
        Ok(img)
    }
}

/// 非圧縮のフレームの大きさから場所を出す。1 bit の画素（バイトの途中でフレームが切れる）や、
/// 画素データがフレームの数に足りないファイルは `None`。
fn native_layout(
    obj: &DefaultDicomObject,
    colors: &Colors,
    value_at: u64,
    length: u32,
    frames: usize,
) -> Option<Layout> {
    let u = |tag| int(obj, tag).unwrap_or(0).max(0) as u64;
    let allocated = u(tags::BITS_ALLOCATED);
    if allocated == 0 || allocated % 8 != 0 {
        return None;
    }
    let (columns, rows) = (u(tags::COLUMNS), u(tags::ROWS));
    let sample = allocated / 8;
    let size = match colors {
        Colors::NativeYbr422 => columns.div_ceil(2) * 4 * sample * rows,
        _ => columns * rows * int(obj, tags::SAMPLES_PER_PIXEL).unwrap_or(1).max(1) as u64 * sample,
    };
    if size == 0 || size.checked_mul(frames as u64)? > u64::from(length) {
        return None;
    }
    let vr = if allocated > 8 { VR::OW } else { VR::OB };
    Some(Layout::Native {
        at: value_at,
        size,
        vr,
    })
}

/// 断片の並びを辿って、フレームごとの断片を出す。読むのは項目のヘッダ（8 バイト）と
/// Basic Offset Table だけ。断片とフレームの対応が決まらなければ `None`。
fn fragment_layout(
    src: &dyn ReadAt,
    value_at: u64,
    frames: usize,
) -> Result<Option<Layout>, DecodeError> {
    let item_header = |at: u64| -> Result<((u16, u16), u32), DecodeError> {
        let mut b = [0u8; 8];
        src.read_at(at, &mut b)?;
        let tag = (
            u16::from_le_bytes([b[0], b[1]]),
            u16::from_le_bytes([b[2], b[3]]),
        );
        Ok((tag, u32::from_le_bytes([b[4], b[5], b[6], b[7]])))
    };
    let (tag, table_len) = item_header(value_at)?;
    if tag != ITEM {
        return Ok(None);
    }
    let mut table = vec![0u8; table_len as usize];
    src.read_at(value_at + 8, &mut table)?;
    let offsets: Vec<u64> = table
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u64::from(u32::from_le_bytes(*b)))
        .collect();
    let first = value_at + 8 + u64::from(table_len);
    let mut fragments = Vec::new();
    let mut at = first;
    while at + 8 <= src.size() {
        let (tag, len) = item_header(at)?;
        match tag {
            SEQUENCE_END => break,
            ITEM => {
                fragments.push((at - first, at + 8, len));
                at += 8 + u64::from(len);
            }
            _ => return Ok(None),
        }
    }
    let grouped: Vec<Vec<(u64, u32)>> = if offsets.len() == frames {
        // 断片は、その項目の位置が、そのフレームの位置から次のフレームの位置の手前までにあるもの。
        (0..frames)
            .map(|i| {
                let end = offsets.get(i + 1).copied().unwrap_or(u64::MAX);
                fragments
                    .iter()
                    .filter(|f| f.0 >= offsets[i] && f.0 < end)
                    .map(|f| (f.1, f.2))
                    .collect()
            })
            .collect()
    } else if fragments.len() == frames {
        fragments.iter().map(|f| vec![(f.1, f.2)]).collect()
    } else {
        return Ok(None);
    };
    if grouped.iter().any(Vec::is_empty) {
        return Ok(None);
    }
    Ok(Some(Layout::Fragments(grouped)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use vw_core::{DecoderPlugin, PixelData};

    use crate::DicomPlugin;
    use crate::synth::{JPEG_LS_LOSSLESS, Synth, XA_IMAGE};

    const BT_IMAGE: &str = "1.2.840.10008.5.1.4.1.1.13.1.3";
    const IMPLICIT_LE: &str = "1.2.840.10008.1.2";

    /// 中身をメモリに持ち、読んだバイト数を数える。
    struct Counted {
        data: Vec<u8>,
        read: Arc<AtomicU64>,
    }

    impl ReadAt for Counted {
        fn size(&self) -> u64 {
            self.data.len() as u64
        }

        fn read_at(&self, at: u64, buf: &mut [u8]) -> std::io::Result<()> {
            let at = at as usize;
            let src = self.data.get(at..at + buf.len()).ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "past the end")
            })?;
            buf.copy_from_slice(src);
            self.read.fetch_add(buf.len() as u64, Ordering::Relaxed);
            Ok(())
        }
    }

    fn counted(data: Vec<u8>) -> (Box<dyn ReadAt>, Arc<AtomicU64>) {
        let read = Arc::new(AtomicU64::new(0));
        (
            Box::new(Counted {
                data,
                read: read.clone(),
            }),
            read,
        )
    }

    fn u16_frame(n: u16, pixels: usize) -> Vec<u16> {
        (0..pixels)
            .map(|i| n.wrapping_mul(1000).wrapping_add(i as u16))
            .collect()
    }

    const SIDE: u16 = 256;
    const PIXELS: usize = SIDE as usize * SIDE as usize;

    /// 1 フレーム 256x256 の 16bit で `frames` 枚。
    fn bt(frames: u16) -> Synth {
        let values: Vec<u16> = (0..frames).flat_map(|n| u16_frame(n, PIXELS)).collect();
        Synth::new(BT_IMAGE, SIDE, SIDE)
            .pixels_u16(&values, 16)
            .text(tags::NUMBER_OF_FRAMES, VR::IS, &frames.to_string())
    }

    fn dbg_of(v: &impl std::fmt::Debug) -> String {
        format!("{v:?}")
    }

    fn u16s(img: &VwImage) -> &[u16] {
        match &img.data {
            PixelData::U16(v) => v,
            other => panic!("expected U16, got {other:?}"),
        }
    }

    #[test]
    fn each_frame_reads_only_its_own_bytes_after_the_header() {
        let bytes = bt(20).bytes();
        let total = bytes.len() as u64;
        let (src, read) = counted(bytes);
        let frames = FileFrames::open(src).unwrap().unwrap();
        assert_eq!(frames.frame_count(), 20);
        let header = read.swap(0, Ordering::Relaxed);
        assert!(header < total / 10, "read {header} of {total} to open");
        for i in [13, 0, 19] {
            let img = frames.decode_frame(i).unwrap();
            assert_eq!(u16s(&img), u16_frame(i as u16, PIXELS));
            assert_eq!(read.swap(0, Ordering::Relaxed), PIXELS as u64 * 2);
        }
    }

    #[test]
    fn frames_read_by_place_match_the_whole_file_reading() {
        for synth in [
            bt(3),
            bt(3).per_frame_values(&[
                (1.0, 0.0, 100.0, 50.0),
                (2.0, -10.0, 900.0, 200.0),
                (1.0, 5.0, 3000.0, 1000.0),
            ]),
        ] {
            let bytes = synth.bytes();
            let (src, _) = counted(bytes.clone());
            let frames = FileFrames::open(src).unwrap().unwrap();
            for i in 0..3 {
                let whole = DicomPlugin.decode_frame(&bytes, i).unwrap();
                let by_place = frames.decode_frame(i).unwrap();
                assert_eq!(dbg_of(&by_place.data), dbg_of(&whole.data), "frame {i}");
                assert_eq!(by_place.window, whole.window, "frame {i}");
                assert_eq!(
                    dbg_of(&by_place.metadata),
                    dbg_of(&whole.metadata),
                    "frame {i}"
                );
            }
        }
    }

    #[test]
    fn implicit_vr_files_are_read_by_place_too() {
        let bytes = bt(2).transfer_syntax(IMPLICIT_LE).bytes();
        let (src, _) = counted(bytes.clone());
        let frames = FileFrames::open(src).unwrap().unwrap();
        let whole = DicomPlugin.decode_frame(&bytes, 1).unwrap();
        assert_eq!(
            dbg_of(&frames.decode_frame(1).unwrap().data),
            dbg_of(&whole.data)
        );
    }

    fn jpeg_ls_frames(n: u8) -> (Vec<Vec<u8>>, Vec<[u8; 16]>) {
        let raw: Vec<[u8; 16]> = (0..n)
            .map(|f| std::array::from_fn(|i| f * 20 + i as u8))
            .collect();
        let jls = raw
            .iter()
            .map(|r| crate::tests::jpeg_ls(4, 4, 8, 1, 0, r))
            .collect();
        (jls, raw)
    }

    #[test]
    fn compressed_frames_are_found_one_fragment_per_frame() {
        let (jls, raw) = jpeg_ls_frames(4);
        let bytes = Synth::new(XA_IMAGE, 4, 4)
            .frames_u8(&[&[0u8; 16][..]; 4])
            .encapsulated(JPEG_LS_LOSSLESS, jls)
            .bytes();
        let (src, read) = counted(bytes);
        let frames = FileFrames::open(src).unwrap().unwrap();
        read.store(0, Ordering::Relaxed);
        let img = frames.decode_frame(2).unwrap();
        assert!(matches!(&img.data, PixelData::U8(v) if v == &raw[2]));
        assert!(read.load(Ordering::Relaxed) < 100);
    }

    #[test]
    fn compressed_frames_split_into_fragments_follow_the_offset_table() {
        let (jls, raw) = jpeg_ls_frames(3);
        let bytes = Synth::new(XA_IMAGE, 4, 4)
            .frames_u8(&[&[0u8; 16][..]; 3])
            .encapsulated_in_halves(JPEG_LS_LOSSLESS, jls)
            .bytes();
        let (src, _) = counted(bytes);
        let frames = FileFrames::open(src).unwrap().unwrap();
        for (i, want) in raw.iter().enumerate() {
            let img = frames.decode_frame(i).unwrap();
            assert!(
                matches!(&img.data, PixelData::U8(v) if v == want),
                "frame {i}"
            );
        }
    }

    #[test]
    fn jpeg_2000_frames_read_by_place_match_the_whole_file_reading() {
        let j2k = include_bytes!("../testdata/gray16.j2k").to_vec();
        let bytes = Synth::new(BT_IMAGE, 4, 2)
            .pixels_u16(&crate::tests::J2K_GRAY16, 16)
            .encapsulated(crate::synth::JPEG_2000_LOSSLESS, vec![j2k.clone(), j2k])
            .bytes();
        let (src, _) = counted(bytes.clone());
        let frames = FileFrames::open(src).unwrap().unwrap();
        let whole = DicomPlugin.decode_frame(&bytes, 1).unwrap();
        assert_eq!(
            dbg_of(&frames.decode_frame(1).unwrap().data),
            dbg_of(&whole.data)
        );
    }

    #[test]
    fn single_frame_files_are_left_to_the_whole_file_reading() {
        let (src, _) = counted(bt(1).bytes());
        assert!(FileFrames::open(src).unwrap().is_none());
    }

    #[test]
    fn a_frame_past_the_end_is_an_error() {
        let (src, _) = counted(bt(2).bytes());
        let frames = FileFrames::open(src).unwrap().unwrap();
        assert!(frames.decode_frame(2).is_err());
    }

    #[test]
    fn the_head_stops_before_the_pixel_values_even_past_the_first_read() {
        // 64 KB を超えるヘッダ。
        let synth = bt(2).text(tags::IMAGE_COMMENTS, VR::UT, &"x".repeat(100_000));
        let bytes = synth.bytes();
        let (src, read) = counted(bytes.clone());
        let head = read_head(&*src).unwrap().unwrap();
        let pixel = head.pixel.unwrap();
        assert_eq!(head.bytes.len() as u64, pixel.value_at);
        assert!(read.load(Ordering::Relaxed) < bytes.len() as u64);
        assert_eq!(
            dbg_of(&DicomPlugin.read_metadata(&head.bytes).unwrap()),
            dbg_of(&DicomPlugin.read_metadata(&bytes).unwrap())
        );
    }
}
