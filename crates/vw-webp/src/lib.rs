//! WebP デコーダプラグイン。
//!
//! - 非可逆（VP8）・可逆（VP8L）とも 8bit の RGB、透過があれば RGBA にする
//! - アニメーションはフレームごとに 1 枚として返す（[`DecoderPlugin::frame_count`]）。
//!   アプリはマルチフレームのファイルと同じく、全フレームを 1 つのスロットに並べる
//! - EXIF をメタデータにし、向きに従って画素を並べ替える

use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

use image_webp::{DecodingError, LoopCount, WebPDecoder};
use vw_core::{ColorModel, DecodeError, DecoderPlugin, Metadata, PixelData, VwImage};

/// アニメーションのフレームを、直前に出したものからいくつ覚えておくか。
const RECENT_FRAMES: usize = 16;
/// 覚えておくフレームの合計の上限（バイト）。大きな画像のアニメーションでメモリを使い過ぎないため。
const RECENT_BYTES: usize = 256 << 20;
/// 表示時間が 10ms 以下のフレームを何ミリ秒として扱うか。GIF（vw-gif）と同じく
/// ブラウザに合わせて 100ms にする。
const SHORT_DELAY_MS: u32 = 100;

#[derive(Default)]
pub struct WebpPlugin {
    /// 最後に読んだアニメーションの、途中まで進めたデコーダ。
    ///
    /// WebP のアニメーションは前のフレームに重ね描きしていくので、n 番目のフレームを
    /// 得るには先頭から n 枚を描く必要がある。フレームを読むたびに先頭からやり直すと、
    /// スライドショーで順に送るだけで全体が枚数の 2 乗の手間になるため、続きから読めるよう
    /// 残しておく。
    animation: Mutex<Option<Animation>>,
}

impl DecoderPlugin for WebpPlugin {
    fn name(&self) -> &str {
        "WebP"
    }

    fn extensions(&self) -> &[&str] {
        &["webp"]
    }

    fn probe(&self, header: &[u8]) -> bool {
        header.len() >= 12 && &header[..4] == b"RIFF" && &header[8..12] == b"WEBP"
    }

    fn decode(&self, data: &[u8]) -> Result<VwImage, DecodeError> {
        self.decode_frame(data, 0)
    }

    fn frame_count(&self, data: &[u8]) -> Result<usize, DecodeError> {
        let decoder = self.headers(data)?;
        Ok(if decoder.is_animated() {
            decoder.num_frames() as usize
        } else {
            1
        })
    }

    fn frame_delays(&self, data: &[u8]) -> Option<Vec<u32>> {
        let delays = anmf_delays(data.get(12..)?);
        (!delays.is_empty()).then_some(delays)
    }

    fn decode_frame(&self, data: &[u8], frame: usize) -> Result<VwImage, DecodeError> {
        let mut decoder = self.headers(data)?;
        let (width, height) = decoder.dimensions();
        let color = if decoder.has_alpha() {
            ColorModel::Rgba
        } else {
            ColorModel::Rgb
        };
        let pixels = if decoder.is_animated() {
            self.animation_frame(data, frame)?
        } else if frame == 0 {
            let mut buf = vec![0; buffer_size(&decoder)?];
            decoder.read_image(&mut buf).map_err(invalid)?;
            buf
        } else {
            return Err(no_frame(frame));
        };
        let mut img = VwImage::new(width, height, color, PixelData::U8(pixels))
            .map_err(DecodeError::Invalid)?;
        let (metadata, orientation) = metadata(&mut decoder);
        img.metadata = metadata;
        img.apply_orientation(orientation);
        Ok(img)
    }

    fn read_metadata(&self, data: &[u8]) -> Result<Metadata, DecodeError> {
        Ok(metadata(&mut self.headers(data)?).0)
    }
}

impl WebpPlugin {
    /// ヘッダまで読んだデコーダ。
    fn headers<'a>(&self, data: &'a [u8]) -> Result<WebPDecoder<Cursor<&'a [u8]>>, DecodeError> {
        // 先に印を確かめるのは、image-webp のエラー（"Invalid RIFF signature" など）では
        // WebP でないファイルだと分かりにくいため。
        if !self.probe(data) {
            return Err(DecodeError::Invalid("not a WebP file".into()));
        }
        WebPDecoder::new(Cursor::new(data)).map_err(invalid)
    }

    fn animation_frame(&self, data: &[u8], frame: usize) -> Result<Vec<u8>, DecodeError> {
        let mut cache = self.animation.lock().unwrap_or_else(|e| e.into_inner());
        if cache.as_ref().is_none_or(|a| *a.data != *data) {
            *cache = Some(Animation::new(data)?);
        }
        let animation = cache.as_mut().expect("set just above");
        let result = animation.frame(frame);
        if result.is_err() {
            // 失敗した途中の状態から続きを読まないよう、次は最初からやり直す。
            *cache = None;
        }
        result
    }
}

/// RIFF の中身（`WEBP` の後）の ANMF チャンクの表示時間（ミリ秒）を順に並べる。
/// image-webp は表示時間をフレームを描いたときにしか返さないので、画素を展開せずに
/// チャンクを読む。
fn anmf_delays(mut body: &[u8]) -> Vec<u32> {
    let mut delays = Vec::new();
    while body.len() >= 8 {
        let size = u32::from_le_bytes([body[4], body[5], body[6], body[7]]) as usize;
        let Some(payload) = body.get(8..8 + size) else {
            break;
        };
        if &body[..4] == b"ANMF" && payload.len() >= 15 {
            let ms = u32::from_le_bytes([payload[12], payload[13], payload[14], 0]);
            delays.push(if ms <= 10 { SHORT_DELAY_MS } else { ms });
        }
        body = body.get(8 + size + size % 2..).unwrap_or_default();
    }
    delays
}

/// 途中まで描いたアニメーション。
struct Animation {
    data: Arc<[u8]>,
    decoder: WebPDecoder<Cursor<Arc<[u8]>>>,
    /// 次に `read_frame` で出てくるフレームの番号。
    next: usize,
    /// 直前に出したフレーム（番号と画素）。ワーカーが前後して頼んだときにやり直さないため。
    recent: VecDeque<(usize, Vec<u8>)>,
    keep: usize,
}

impl Animation {
    fn new(data: &[u8]) -> Result<Self, DecodeError> {
        let data: Arc<[u8]> = Arc::from(data);
        let mut decoder = WebPDecoder::new(Cursor::new(data.clone())).map_err(invalid)?;
        // 透明で消す。ファイルの背景色（ANIM の色）を使わないのは、ブラウザも libwebp も
        // 透明で消していて、仕様でも背景色は参考に留めてよいとされているため。
        // 何も指定しないと image-webp は消さずに残すので、前のフレームの絵が残ってしまう。
        decoder
            .set_background_color([0, 0, 0, 0])
            .map_err(invalid)?;
        let size = buffer_size(&decoder)?;
        let keep = (RECENT_BYTES / size.max(1)).clamp(1, RECENT_FRAMES);
        Ok(Self {
            data,
            decoder,
            next: 0,
            recent: VecDeque::new(),
            keep,
        })
    }

    fn frame(&mut self, frame: usize) -> Result<Vec<u8>, DecodeError> {
        if let Some((_, pixels)) = self.recent.iter().find(|(i, _)| *i == frame) {
            return Ok(pixels.clone());
        }
        if frame >= self.decoder.num_frames() as usize {
            return Err(no_frame(frame));
        }
        if frame < self.next {
            self.decoder.reset_animation();
            self.next = 0;
        }
        let mut buf = vec![0; buffer_size(&self.decoder)?];
        while self.next <= frame {
            self.decoder.read_frame(&mut buf).map_err(invalid)?;
            if self.recent.len() == self.keep {
                self.recent.pop_front();
            }
            self.recent.push_back((self.next, buf.clone()));
            self.next += 1;
        }
        Ok(buf)
    }
}

fn buffer_size<R: std::io::BufRead + std::io::Seek>(
    decoder: &WebPDecoder<R>,
) -> Result<usize, DecodeError> {
    let size = decoder
        .output_buffer_size()
        .ok_or_else(|| DecodeError::Unsupported("WebP image too large".into()))?;
    vw_core::limits::check_bytes(size as u64, "WebP image")
}

fn invalid(e: DecodingError) -> DecodeError {
    DecodeError::Invalid(e.to_string())
}

fn no_frame(frame: usize) -> DecodeError {
    DecodeError::Invalid(format!("no frame {frame}"))
}

/// WebP のヘッダと EXIF を表示用のメタデータにし、EXIF の向きと一緒に返す（なければ 1）。
fn metadata<R: std::io::BufRead + std::io::Seek>(decoder: &mut WebPDecoder<R>) -> (Metadata, u32) {
    let mut m = Metadata::default();
    let (w, h) = decoder.dimensions();
    m.push("WebP", "Size", format!("{w} x {h}"));
    let coding = if decoder.is_lossy() {
        "Lossy (VP8)"
    } else {
        "Lossless (VP8L)"
    };
    m.push("WebP", "Coding", coding);
    m.push(
        "WebP",
        "Alpha",
        if decoder.has_alpha() { "yes" } else { "no" },
    );
    if let Ok(Some(_)) = decoder.icc_profile() {
        m.push("WebP", "ICC profile", "present");
    }
    if decoder.is_animated() {
        let frames = decoder.num_frames();
        m.push("Animation", "Frames", frames);
        let loops = match decoder.loop_count() {
            LoopCount::Forever => "forever".to_string(),
            LoopCount::Times(n) => n.to_string(),
        };
        m.push("Animation", "Loop count", loops);
        if frames > 0 && decoder.loop_duration() > 0 {
            let ms = decoder.loop_duration() as f64 / f64::from(frames);
            m.push("Animation", "Frame time (ms)", format!("{ms:.1}"));
        }
    }
    let orientation = match decoder.exif_metadata() {
        Ok(Some(raw)) => vw_core::exif::push_exif(&mut m, raw),
        _ => 1,
    };
    (m, orientation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image_webp::{ColorType, WebPEncoder};
    use vw_core::exif::sample;

    /// 可逆の WebP。`exif` があれば EXIF チャンクを付ける。
    fn encode(w: u32, h: u32, color: ColorType, data: &[u8], exif: Option<Vec<u8>>) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = WebPEncoder::new(&mut out);
        if let Some(exif) = exif {
            enc.set_exif_metadata(exif);
        }
        enc.encode(data, w, h, color).unwrap();
        out
    }

    fn chunk(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut c = fourcc.to_vec();
        c.extend((payload.len() as u32).to_le_bytes());
        c.extend(payload);
        if payload.len() % 2 == 1 {
            c.push(0);
        }
        c
    }

    fn u24(v: u32) -> [u8; 3] {
        let b = v.to_le_bytes();
        [b[0], b[1], b[2]]
    }

    /// `w` x `h` の、単色のフレームを重ねたアニメーション WebP。各フレームはキャンバス全体を
    /// 覆い、表示時間は `ms`。
    fn animation(w: u32, h: u32, colors: &[[u8; 3]], ms: u32) -> Vec<u8> {
        let frames: Vec<Frame> = colors
            .iter()
            .map(|&color| Frame {
                x: 0,
                y: 0,
                w,
                h,
                color,
                dispose: false,
            })
            .collect();
        animation_of(w, h, &frames, ms)
    }

    /// アニメーションの 1 フレーム。`dispose` なら、次のフレームを描く前にこの範囲を消す。
    struct Frame {
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        color: [u8; 3],
        dispose: bool,
    }

    /// image-webp はアニメーションを書けないので、チャンクを手で組む。
    fn animation_of(w: u32, h: u32, frames: &[Frame], ms: u32) -> Vec<u8> {
        animation_timed(w, h, frames, &vec![ms; frames.len()])
    }

    /// フレームごとに表示時間 `ms` の違うアニメーション。
    fn animation_timed(w: u32, h: u32, frames: &[Frame], ms: &[u32]) -> Vec<u8> {
        let mut body = b"WEBP".to_vec();
        let mut vp8x = vec![0x02, 0, 0, 0]; // アニメーションあり
        vp8x.extend(u24(w - 1));
        vp8x.extend(u24(h - 1));
        body.extend(chunk(b"VP8X", &vp8x));
        body.extend(chunk(b"ANIM", &[0, 0, 0, 0, 0, 0])); // 背景色、無限に繰り返す
        for (f, &ms) in frames.iter().zip(ms) {
            let pixels = f.color.repeat((f.w * f.h) as usize);
            let still = encode(f.w, f.h, ColorType::Rgb8, &pixels, None);
            let mut anmf = Vec::new();
            anmf.extend(u24(f.x / 2));
            anmf.extend(u24(f.y / 2));
            anmf.extend(u24(f.w - 1));
            anmf.extend(u24(f.h - 1));
            anmf.extend(u24(ms));
            anmf.push(0b10 | u8::from(f.dispose)); // 重ねずに上書き
            anmf.extend(&still[12..]); // RIFF ヘッダを除いた VP8L チャンク
            body.extend(chunk(b"ANMF", &anmf));
        }
        let mut out = b"RIFF".to_vec();
        out.extend((body.len() as u32).to_le_bytes());
        out.extend(body);
        out
    }

    fn first_pixel(img: &VwImage) -> &[u8] {
        let PixelData::U8(v) = &img.data else {
            panic!("8bit expected")
        };
        &v[..img.color.channels()]
    }

    const RED: [u8; 3] = [255, 0, 0];
    const GREEN: [u8; 3] = [0, 255, 0];
    const BLUE: [u8; 3] = [0, 0, 255];

    #[test]
    fn each_animation_frame_keeps_its_own_delay() {
        let frame = |color| Frame {
            x: 0,
            y: 0,
            w: 2,
            h: 2,
            color,
            dispose: false,
        };
        let webp = animation_timed(
            2,
            2,
            &[frame(RED), frame(GREEN), frame(BLUE)],
            &[40, 0, 250],
        );
        assert_eq!(
            WebpPlugin::default().frame_delays(&webp),
            Some(vec![40, 100, 250])
        );
    }

    #[test]
    fn a_still_webp_has_no_frame_delays() {
        let still = encode(1, 1, ColorType::Rgb8, &RED, None);
        assert_eq!(WebpPlugin::default().frame_delays(&still), None);
    }

    #[test]
    fn probe_accepts_riff_webp_only() {
        assert!(WebpPlugin::default().probe(b"RIFF\x10\0\0\0WEBPVP8L"));
        assert!(!WebpPlugin::default().probe(b"RIFF\x10\0\0\0WAVEfmt "));
        assert!(!WebpPlugin::default().probe(b"\x89PNG\r\n\x1a\n"));
    }

    #[test]
    fn decodes_opaque_webp_as_rgb8() {
        let webp = encode(4, 2, ColorType::Rgb8, &RED.repeat(8), None);
        let img = WebpPlugin::default().decode(&webp).unwrap();
        assert_eq!((img.width, img.height, img.color), (4, 2, ColorModel::Rgb));
        assert_eq!(first_pixel(&img), RED);
        assert_eq!(img.metadata.get("WebP", "Coding"), Some("Lossless (VP8L)"));
    }

    #[test]
    fn keeps_transparency_as_rgba8() {
        let webp = encode(2, 2, ColorType::Rgba8, &[0, 0, 255, 128].repeat(4), None);
        let img = WebpPlugin::default().decode(&webp).unwrap();
        assert_eq!(img.color, ColorModel::Rgba);
        assert_eq!(first_pixel(&img), [0, 0, 255, 128]);
    }

    #[test]
    fn exif_orientation_turns_the_image_upright() {
        let webp = encode(
            4,
            2,
            ColorType::Rgb8,
            &RED.repeat(8),
            Some(sample::orientation(6)),
        );
        let img = WebpPlugin::default().decode(&webp).unwrap();
        assert_eq!((img.width, img.height), (2, 4));
        assert!(img.metadata.get("EXIF", "Orientation").is_some());
    }

    #[test]
    fn still_image_has_one_frame() {
        let webp = encode(2, 2, ColorType::Rgb8, &RED.repeat(4), None);
        let plugin = WebpPlugin::default();
        assert_eq!(plugin.frame_count(&webp).unwrap(), 1);
        assert!(plugin.decode_frame(&webp, 1).is_err());
    }

    #[test]
    fn animation_frames_are_separate_images() {
        let webp = animation(3, 2, &[RED, GREEN, BLUE], 40);
        let plugin = WebpPlugin::default();
        assert_eq!(plugin.frame_count(&webp).unwrap(), 3);
        for (i, color) in [RED, GREEN, BLUE].iter().enumerate() {
            let img = plugin.decode_frame(&webp, i).unwrap();
            assert_eq!((img.width, img.height), (3, 2));
            assert_eq!(&first_pixel(&img)[..3], color, "frame {i}");
        }
        assert!(plugin.decode_frame(&webp, 3).is_err());
    }

    #[test]
    fn animation_frames_can_be_read_in_any_order() {
        let webp = animation(2, 2, &[RED, GREEN, BLUE], 40);
        let plugin = WebpPlugin::default();
        for (i, color) in [(2, BLUE), (0, RED), (1, GREEN), (2, BLUE), (0, RED)] {
            let img = plugin.decode_frame(&webp, i).unwrap();
            assert_eq!(&first_pixel(&img)[..3], color, "frame {i}");
        }
    }

    #[test]
    fn going_back_past_the_remembered_frames_restarts_the_animation() {
        let colors: Vec<[u8; 3]> = (0..RECENT_FRAMES as u8 + 4).map(|i| [i, 0, 0]).collect();
        let webp = animation(2, 2, &colors, 40);
        let plugin = WebpPlugin::default();
        let last = colors.len() - 1;
        assert_eq!(
            first_pixel(&plugin.decode_frame(&webp, last).unwrap())[0],
            last as u8
        );
        assert_eq!(first_pixel(&plugin.decode_frame(&webp, 1).unwrap())[0], 1);
    }

    #[test]
    fn disposed_frame_is_cleared_before_the_next_one() {
        let frames = [
            Frame {
                x: 0,
                y: 0,
                w: 4,
                h: 2,
                color: RED,
                dispose: true,
            },
            Frame {
                x: 2,
                y: 0,
                w: 2,
                h: 2,
                color: GREEN,
                dispose: false,
            },
        ];
        let webp = animation_of(4, 2, &frames, 40);
        let img = WebpPlugin::default().decode_frame(&webp, 1).unwrap();
        let PixelData::U8(v) = &img.data else {
            panic!("8bit expected")
        };
        let n = img.color.channels();
        assert_eq!(&v[..3], [0, 0, 0], "the red frame is gone");
        assert_eq!(&v[2 * n..2 * n + 3], GREEN);
    }

    #[test]
    fn animation_metadata_gives_frames_and_frame_time() {
        let webp = animation(2, 2, &[RED, GREEN], 50);
        let m = WebpPlugin::default().read_metadata(&webp).unwrap();
        assert_eq!(m.get("Animation", "Frames"), Some("2"));
        assert_eq!(m.get("Animation", "Frame time (ms)"), Some("50.0"));
        assert_eq!(m.get("Animation", "Loop count"), Some("forever"));
    }

    #[test]
    fn file_that_is_not_webp_says_so() {
        let err = WebpPlugin::default()
            .decode(b"this is not an image\n")
            .unwrap_err();
        assert_eq!(err.to_string(), "invalid data: not a WebP file");
    }

    #[test]
    fn truncated_file_is_an_error() {
        let webp = encode(8, 8, ColorType::Rgb8, &RED.repeat(64), None);
        assert!(WebpPlugin::default().decode(&webp[..20]).is_err());
    }
}
