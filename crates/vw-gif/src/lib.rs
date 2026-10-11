//! GIF デコーダプラグイン。
//!
//! - 8bit の RGB、透過があれば RGBA にする
//! - アニメーションはフレームごとに 1 枚として返す（[`DecoderPlugin::frame_count`]）。
//!   アプリはマルチフレームのファイルと同じく、全フレームを 1 つのスロットに並べる
//! - フレームは前のフレームに重ね描きするので、キャンバスに合成した結果を返す

use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

use gif::{
    ColorOutput, DecodeOptions, Decoder, DecodingError, DisposalMethod, MemoryLimit, Repeat,
};
use vw_core::{ColorModel, DecodeError, DecoderPlugin, Metadata, PixelData, VwImage};

/// アニメーションのフレームを、直前に出したものからいくつ覚えておくか。
const RECENT_FRAMES: usize = 16;
/// 覚えておくフレームの合計の上限（バイト）。大きな画像のアニメーションでメモリを使い過ぎないため。
const RECENT_BYTES: usize = 256 << 20;
/// 1 フレームの展開に使ってよいメモリ。gif の既定（50MB）では 4000 x 4000 の写真の GIF も
/// 読めないので広げる。上限を無くさないのは、小さなファイルが巨大なフレームを名乗る
/// 展開爆弾を防ぐため。
const FRAME_MEMORY: u64 = 1 << 30;
/// 表示時間が 0 か 1（10ms 以下）のフレームを何ミリ秒として扱うか。ブラウザと同じく
/// 100ms にする。書かれたとおりに受け取ると、古い GIF が意図より何倍も速く回る。
const SHORT_DELAY_MS: u32 = 100;

#[derive(Default)]
pub struct GifPlugin {
    /// 最後に読んだアニメーションの、途中まで進めたキャンバス。
    ///
    /// n 番目のフレームを得るには先頭から n 枚を重ねる必要がある。フレームを読むたびに
    /// 先頭からやり直すと、スライドショーで順に送るだけで全体が枚数の 2 乗の手間になるため、
    /// 続きから読めるよう残しておく。
    animation: Mutex<Option<Animation>>,
}

impl DecoderPlugin for GifPlugin {
    fn name(&self) -> &str {
        "GIF"
    }

    fn extensions(&self) -> &[&str] {
        &["gif"]
    }

    fn probe(&self, header: &[u8]) -> bool {
        header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a")
    }

    fn decode(&self, data: &[u8]) -> Result<VwImage, DecodeError> {
        self.decode_frame(data, 0)
    }

    fn frame_count(&self, data: &[u8]) -> Result<usize, DecodeError> {
        Ok(self.summary(data)?.frames)
    }

    fn decode_frame(&self, data: &[u8], frame: usize) -> Result<VwImage, DecodeError> {
        let mut cache = self.animation.lock().unwrap_or_else(|e| e.into_inner());
        if !cache.as_ref().is_some_and(|a| *a.data == *data) {
            *cache = Some(Animation::new(data, self.summary(data)?)?);
        }
        let animation = cache.as_mut().expect("set just above");
        let result = animation.image(frame);
        if result.is_err() {
            // 失敗した途中の状態から続きを読まないよう、次は最初からやり直す。
            *cache = None;
        }
        result
    }

    fn read_metadata(&self, data: &[u8]) -> Result<Metadata, DecodeError> {
        Ok(self.summary(data)?.metadata())
    }
}

impl GifPlugin {
    fn summary(&self, data: &[u8]) -> Result<Summary, DecodeError> {
        // 先に印を確かめるのは、gif のエラーでは GIF でないファイルだと分かりにくいため。
        if !self.probe(data) {
            return Err(DecodeError::Invalid("not a GIF file".into()));
        }
        Summary::scan(data)
    }
}

/// フレームの画素を展開せずに、ファイル全体を一度なめて分かること。
struct Summary {
    width: u32,
    height: u32,
    frames: usize,
    total_delay_ms: u64,
    /// 透明な画素があり得るか。透明色を持つフレーム、キャンバスを覆いきらない最初の
    /// フレーム、透明に消すフレームのどれかがあれば RGBA にする。フレームごとに RGB / RGBA を変えないのは、同じスロットで
    /// 送ったときに見え方の扱いが変わらないようにするため。
    alpha: bool,
    repeat: Repeat,
    global_colors: Option<usize>,
    interlaced: bool,
}

impl Summary {
    fn scan(data: &[u8]) -> Result<Self, DecodeError> {
        let mut options = DecodeOptions::new();
        options.skip_frame_decoding(true);
        let mut decoder = options.read_info(Cursor::new(data)).map_err(invalid)?;
        let (width, height) = (u32::from(decoder.width()), u32::from(decoder.height()));
        let global_colors = decoder.global_palette().map(|p| p.len() / 3);
        let mut s = Self {
            width,
            height,
            frames: 0,
            total_delay_ms: 0,
            alpha: false,
            repeat: Repeat::Finite(0),
            global_colors,
            interlaced: false,
        };
        while let Some(f) = decoder.read_next_frame().map_err(invalid)? {
            let covers = f.left == 0
                && f.top == 0
                && u32::from(f.width) >= width
                && u32::from(f.height) >= height;
            s.alpha |= f.transparent.is_some()
                || (s.frames == 0 && !covers)
                || f.dispose == DisposalMethod::Background;
            s.interlaced |= f.interlaced;
            s.total_delay_ms += u64::from(delay_ms(f.delay));
            s.frames += 1;
        }
        if s.frames == 0 {
            return Err(DecodeError::Invalid("GIF has no frames".into()));
        }
        s.repeat = decoder.repeat();
        Ok(s)
    }

    fn metadata(&self) -> Metadata {
        let mut m = Metadata::default();
        m.push("GIF", "Size", format!("{} x {}", self.width, self.height));
        if let Some(n) = self.global_colors {
            m.push("GIF", "Global colors", n);
        }
        m.push("GIF", "Transparency", if self.alpha { "yes" } else { "no" });
        m.push(
            "GIF",
            "Interlaced",
            if self.interlaced { "yes" } else { "no" },
        );
        if self.frames > 1 {
            m.push("Animation", "Frames", self.frames);
            let loops = match self.repeat {
                Repeat::Infinite => "forever".to_string(),
                Repeat::Finite(0) => "once".to_string(),
                Repeat::Finite(n) => n.to_string(),
            };
            m.push("Animation", "Loop count", loops);
            let ms = self.total_delay_ms as f64 / self.frames as f64;
            m.push("Animation", "Frame time (ms)", format!("{ms:.1}"));
        }
        m
    }
}

fn delay_ms(centiseconds: u16) -> u32 {
    if centiseconds <= 1 {
        SHORT_DELAY_MS
    } else {
        u32::from(centiseconds) * 10
    }
}

/// 途中まで重ねたアニメーション。
struct Animation {
    data: Arc<[u8]>,
    summary: Summary,
    decoder: Decoder<Cursor<Arc<[u8]>>>,
    /// RGBA のキャンバス。直前に出したフレームまでを重ねたもの。
    canvas: Vec<u8>,
    /// 直前のフレームを、次のフレームを描く前にどう片付けるか。
    dispose: Option<Dispose>,
    /// 次に読むフレームの番号。
    next: usize,
    /// 直前に出したフレーム（番号と画素）。ワーカーが前後して頼んだときにやり直さないため。
    recent: VecDeque<(usize, Vec<u8>)>,
    keep: usize,
}

struct Dispose {
    rect: Rect,
    method: DisposalMethod,
    /// `Previous` のときの、描く前のその範囲の画素。
    saved: Vec<u8>,
}

#[derive(Clone, Copy)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl Animation {
    fn new(data: &[u8], summary: Summary) -> Result<Self, DecodeError> {
        let data: Arc<[u8]> = Arc::from(data);
        let size = canvas_size(summary.width, summary.height)?;
        let decoder = open(&data)?;
        let keep = (RECENT_BYTES / size.max(1)).clamp(1, RECENT_FRAMES);
        Ok(Self {
            data,
            summary,
            decoder,
            canvas: vec![0; size],
            dispose: None,
            next: 0,
            recent: VecDeque::new(),
            keep,
        })
    }

    fn image(&mut self, frame: usize) -> Result<VwImage, DecodeError> {
        let rgba = self.frame(frame)?;
        let s = &self.summary;
        let (color, pixels) = if s.alpha {
            (ColorModel::Rgba, rgba)
        } else {
            let rgb = rgba
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2]]);
            (ColorModel::Rgb, rgb.collect())
        };
        let mut img = VwImage::new(s.width, s.height, color, PixelData::U8(pixels))
            .map_err(DecodeError::Invalid)?;
        img.metadata = s.metadata();
        Ok(img)
    }

    fn frame(&mut self, frame: usize) -> Result<Vec<u8>, DecodeError> {
        if let Some((_, pixels)) = self.recent.iter().find(|(i, _)| *i == frame) {
            return Ok(pixels.clone());
        }
        if frame >= self.summary.frames {
            return Err(DecodeError::Invalid(format!("no frame {frame}")));
        }
        if frame < self.next {
            // gif のデコーダは巻き戻せないので開き直す。
            self.decoder = open(&self.data)?;
            self.canvas.fill(0);
            self.dispose = None;
            self.next = 0;
        }
        while self.next <= frame {
            self.draw_next()?;
            if self.recent.len() == self.keep {
                self.recent.pop_front();
            }
            self.recent.push_back((self.next, self.canvas.clone()));
            self.next += 1;
        }
        Ok(self.canvas.clone())
    }

    fn draw_next(&mut self) -> Result<(), DecodeError> {
        let stride = self.summary.width as usize * 4;
        if let Some(d) = self.dispose.take() {
            match d.method {
                // 背景色で塗らずに透明にするのは、ブラウザがそうしていて、GIF を作る側も
                // それを前提にしているため。
                DisposalMethod::Background => {
                    for row in d.rect.rows(stride) {
                        self.canvas[row].fill(0);
                    }
                }
                DisposalMethod::Previous => {
                    let line = d.rect.w * 4;
                    for (row, saved) in d.rect.rows(stride).zip(d.saved.chunks_exact(line)) {
                        self.canvas[row].copy_from_slice(saved);
                    }
                }
                DisposalMethod::Keep | DisposalMethod::Any => {}
            }
        }
        let f = self
            .decoder
            .read_next_frame()
            .map_err(invalid)?
            .ok_or_else(|| DecodeError::Invalid("GIF ended early".into()))?;
        let fw = usize::from(f.width);
        // キャンバスからはみ出す部分は描かない。
        let rect = Rect {
            x: usize::from(f.left),
            y: usize::from(f.top),
            w: fw,
            h: usize::from(f.height),
        }
        .clip(self.summary.width as usize, self.summary.height as usize);
        let saved = if f.dispose == DisposalMethod::Previous {
            rect.rows(stride)
                .flat_map(|row| self.canvas[row].to_vec())
                .collect()
        } else {
            Vec::new()
        };
        for (dy, row) in rect.rows(stride).enumerate() {
            let src = &f.buffer[dy * fw * 4..][..rect.w * 4];
            let dst = &mut self.canvas[row];
            for (d, s) in dst
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(src.as_chunks::<4>().0)
            {
                if s[3] != 0 {
                    *d = *s;
                }
            }
        }
        self.dispose = Some(Dispose {
            rect,
            method: f.dispose,
            saved,
        });
        Ok(())
    }
}

impl Rect {
    fn clip(self, width: usize, height: usize) -> Self {
        let x = self.x.min(width);
        let y = self.y.min(height);
        Self {
            x,
            y,
            w: self.w.min(width - x),
            h: self.h.min(height - y),
        }
    }

    /// キャンバス（1 行 `stride` バイト）の中で、この範囲の各行が占めるバイトの範囲。
    fn rows(self, stride: usize) -> impl Iterator<Item = std::ops::Range<usize>> {
        (self.y..self.y + self.h).map(move |y| {
            let start = y * stride + self.x * 4;
            start..start + self.w * 4
        })
    }
}

fn open(data: &Arc<[u8]>) -> Result<Decoder<Cursor<Arc<[u8]>>>, DecodeError> {
    let mut options = DecodeOptions::new();
    options.set_color_output(ColorOutput::RGBA);
    options.set_memory_limit(MemoryLimit::Bytes(
        FRAME_MEMORY.try_into().expect("non-zero"),
    ));
    options
        .read_info(Cursor::new(data.clone()))
        .map_err(invalid)
}

fn canvas_size(width: u32, height: u32) -> Result<usize, DecodeError> {
    let size = width as u64 * height as u64 * 4;
    if size == 0 || size > FRAME_MEMORY {
        return Err(DecodeError::Unsupported(format!(
            "GIF canvas {width} x {height} is too large or empty"
        )));
    }
    Ok(size as usize)
}

fn invalid(e: DecodingError) -> DecodeError {
    DecodeError::Invalid(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gif::{Encoder, Frame};

    const RED: [u8; 3] = [255, 0, 0];
    const GREEN: [u8; 3] = [0, 255, 0];
    const BLUE: [u8; 3] = [0, 0, 255];
    /// 0 = 赤、1 = 緑、2 = 青、3 = 黒の全体パレット。
    const PALETTE: [u8; 12] = [255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0];

    /// パレットの番号で塗ったフレーム。
    struct F {
        rect: (u16, u16, u16, u16),
        index: u8,
        delay: u16,
        dispose: DisposalMethod,
        transparent: Option<u8>,
    }

    fn full(w: u16, h: u16, index: u8, delay: u16) -> F {
        F {
            rect: (0, 0, w, h),
            index,
            delay,
            dispose: DisposalMethod::Keep,
            transparent: None,
        }
    }

    fn encode(w: u16, h: u16, frames: &[F], repeat: Option<Repeat>) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = Encoder::new(&mut out, w, h, &PALETTE).unwrap();
            if let Some(r) = repeat {
                enc.set_repeat(r).unwrap();
            }
            for f in frames {
                let (left, top, fw, fh) = f.rect;
                let mut frame = Frame {
                    left,
                    top,
                    width: fw,
                    height: fh,
                    delay: f.delay,
                    dispose: f.dispose,
                    transparent: f.transparent,
                    ..Frame::default()
                };
                frame.buffer = vec![f.index; fw as usize * fh as usize].into();
                enc.write_frame(&frame).unwrap();
            }
        }
        out
    }

    fn pixel(img: &VwImage, x: usize, y: usize) -> &[u8] {
        let PixelData::U8(v) = &img.data else {
            panic!("8bit expected")
        };
        let n = img.color.channels();
        &v[(y * img.width as usize + x) * n..][..n]
    }

    #[test]
    fn probe_accepts_both_gif_versions_only() {
        let plugin = GifPlugin::default();
        assert!(plugin.probe(b"GIF87a\x01\0"));
        assert!(plugin.probe(b"GIF89a\x01\0"));
        assert!(!plugin.probe(b"\x89PNG\r\n\x1a\n"));
    }

    #[test]
    fn opaque_still_gif_is_rgb8_with_one_frame() {
        let gif = encode(3, 2, &[full(3, 2, 2, 0)], None);
        let plugin = GifPlugin::default();
        let img = plugin.decode(&gif).unwrap();
        assert_eq!((img.width, img.height, img.color), (3, 2, ColorModel::Rgb));
        assert_eq!(pixel(&img, 2, 1), BLUE);
        assert_eq!(plugin.frame_count(&gif).unwrap(), 1);
        assert!(plugin.decode_frame(&gif, 1).is_err());
        assert_eq!(img.metadata.get("GIF", "Size"), Some("3 x 2"));
        assert_eq!(img.metadata.get("Animation", "Frames"), None);
    }

    #[test]
    fn transparent_colour_becomes_alpha_zero() {
        let mut f = full(2, 1, 0, 0);
        f.transparent = Some(0);
        let img = GifPlugin::default()
            .decode(&encode(2, 1, &[f], None))
            .unwrap();
        assert_eq!(img.color, ColorModel::Rgba);
        assert_eq!(pixel(&img, 0, 0)[3], 0);
    }

    #[test]
    fn animation_frames_are_separate_images_in_any_order() {
        let frames = [full(2, 2, 0, 5), full(2, 2, 1, 5), full(2, 2, 2, 5)];
        let gif = encode(2, 2, &frames, Some(Repeat::Infinite));
        let plugin = GifPlugin::default();
        assert_eq!(plugin.frame_count(&gif).unwrap(), 3);
        for (i, color) in [(2, BLUE), (0, RED), (1, GREEN), (2, BLUE), (0, RED)] {
            let img = plugin.decode_frame(&gif, i).unwrap();
            assert_eq!(pixel(&img, 1, 1), color, "frame {i}");
        }
        assert!(plugin.decode_frame(&gif, 3).is_err());
    }

    #[test]
    fn going_back_past_the_remembered_frames_restarts_the_animation() {
        let frames: Vec<F> = (0..RECENT_FRAMES + 4)
            .map(|i| full(1, 1, (i % 3) as u8, 5))
            .collect();
        let gif = encode(1, 1, &frames, None);
        let plugin = GifPlugin::default();
        let last = frames.len() - 1;
        let img = plugin.decode_frame(&gif, last).unwrap();
        assert_eq!(pixel(&img, 0, 0), [RED, GREEN, BLUE][last % 3]);
        assert_eq!(pixel(&plugin.decode_frame(&gif, 1).unwrap(), 0, 0), GREEN);
    }

    #[test]
    fn smaller_frames_are_drawn_over_the_previous_one() {
        let frames = [
            full(4, 1, 0, 5),
            F {
                rect: (2, 0, 2, 1),
                ..full(2, 1, 1, 5)
            },
        ];
        let img = GifPlugin::default()
            .decode_frame(&encode(4, 1, &frames, None), 1)
            .unwrap();
        assert_eq!(pixel(&img, 0, 0), RED);
        assert_eq!(pixel(&img, 3, 0), GREEN);
    }

    #[test]
    fn transparent_pixels_let_the_previous_frame_show_through() {
        let mut top = full(2, 1, 3, 5);
        top.transparent = Some(3);
        let frames = [full(2, 1, 2, 5), top];
        let img = GifPlugin::default()
            .decode_frame(&encode(2, 1, &frames, None), 1)
            .unwrap();
        assert_eq!(&pixel(&img, 0, 0)[..3], BLUE);
    }

    #[test]
    fn background_disposal_clears_the_frame_to_transparent() {
        let mut first = full(4, 1, 0, 5);
        first.dispose = DisposalMethod::Background;
        let frames = [
            first,
            F {
                rect: (2, 0, 2, 1),
                ..full(2, 1, 1, 5)
            },
        ];
        let img = GifPlugin::default()
            .decode_frame(&encode(4, 1, &frames, None), 1)
            .unwrap();
        assert_eq!(img.color, ColorModel::Rgba);
        assert_eq!(pixel(&img, 0, 0)[3], 0);
        assert_eq!(&pixel(&img, 3, 0)[..3], GREEN);
    }

    #[test]
    fn previous_disposal_restores_what_was_under_the_frame() {
        let mut middle = F {
            rect: (0, 0, 1, 1),
            ..full(1, 1, 1, 5)
        };
        middle.dispose = DisposalMethod::Previous;
        let last = F {
            rect: (1, 0, 1, 1),
            ..full(1, 1, 2, 5)
        };
        let frames = [full(2, 1, 0, 5), middle, last];
        let img = GifPlugin::default()
            .decode_frame(&encode(2, 1, &frames, None), 2)
            .unwrap();
        assert_eq!(pixel(&img, 0, 0), RED);
        assert_eq!(pixel(&img, 1, 0), BLUE);
    }

    #[test]
    fn frames_reaching_outside_the_canvas_are_clipped() {
        let frames = [F {
            rect: (1, 1, 4, 4),
            ..full(4, 4, 1, 0)
        }];
        let img = GifPlugin::default()
            .decode(&encode(2, 2, &frames, None))
            .unwrap();
        assert_eq!(&pixel(&img, 1, 1)[..3], GREEN);
        assert_eq!(pixel(&img, 0, 0)[3], 0);
    }

    #[test]
    fn animation_metadata_gives_frames_loops_and_frame_time() {
        let frames = [full(1, 1, 0, 4), full(1, 1, 1, 8)];
        let gif = encode(1, 1, &frames, Some(Repeat::Infinite));
        let m = GifPlugin::default().read_metadata(&gif).unwrap();
        assert_eq!(m.get("Animation", "Frames"), Some("2"));
        assert_eq!(m.get("Animation", "Frame time (ms)"), Some("60.0"));
        assert_eq!(m.get("Animation", "Loop count"), Some("forever"));
    }

    #[test]
    fn zero_delay_is_shown_at_the_browser_speed() {
        let frames = [full(1, 1, 0, 0), full(1, 1, 1, 1)];
        let m = GifPlugin::default()
            .read_metadata(&encode(1, 1, &frames, None))
            .unwrap();
        assert_eq!(m.get("Animation", "Frame time (ms)"), Some("100.0"));
    }

    #[test]
    fn file_that_is_not_gif_says_so() {
        let err = GifPlugin::default()
            .decode(b"this is not an image\n")
            .unwrap_err();
        assert_eq!(err.to_string(), "invalid data: not a GIF file");
    }

    #[test]
    fn truncated_file_is_an_error() {
        let gif = encode(8, 8, &[full(8, 8, 0, 0), full(8, 8, 1, 0)], None);
        let plugin = GifPlugin::default();
        assert!(plugin.decode(&gif[..20]).is_err());
    }
}
