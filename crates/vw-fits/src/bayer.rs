//! Bayer のモザイク（1 画素 1 色）を RGB にする。
//!
//! 周りの同じ色の画素の平均で埋める双線形の補間。縁の色ずれは出るが、撮影中のピントや
//! 雲の確認には足りる。AHD などの精細な方式は、計算が重く 1 枚目が出るまで待たせるので採らない。

/// 2x2 の並びの色。0 = 赤、1 = 緑、2 = 青。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pattern([u8; 4]);

impl Pattern {
    /// `BAYERPAT` の値（`RGGB` など）と、`XBAYROFF`・`YBAYROFF` のずれ。ずれた分だけ並びを動かす。
    pub fn new(name: &str, x_offset: i64, y_offset: i64) -> Option<Self> {
        let name = name.trim().to_ascii_uppercase();
        let colors: Vec<u8> = name
            .bytes()
            .map(|c| match c {
                b'R' => Some(0),
                b'G' => Some(1),
                b'B' => Some(2),
                _ => None,
            })
            .collect::<Option<_>>()?;
        let base: [u8; 4] = colors.try_into().ok()?;
        let (dx, dy) = (
            x_offset.rem_euclid(2) as usize,
            y_offset.rem_euclid(2) as usize,
        );
        let mut out = [0; 4];
        for y in 0..2 {
            for x in 0..2 {
                out[y * 2 + x] = base[((y + dy) % 2) * 2 + (x + dx) % 2];
            }
        }
        // 緑が 2 つ、赤と青が 1 つずつの並びだけを受け付ける。
        let count = |c| out.iter().filter(|&&o| o == c).count();
        (count(0) == 1 && count(1) == 2 && count(2) == 1).then_some(Self(out))
    }

    pub fn color_at(self, x: usize, y: usize) -> u8 {
        self.0[(y % 2) * 2 + x % 2]
    }
}

/// 幅 `w`、高さ `h` のモザイク `src` を、RGB の並び（画素ごとに 3 値）にする。
pub fn demosaic(src: &[f32], w: usize, h: usize, pattern: Pattern) -> Vec<f32> {
    let mut out = vec![0.0; w * h * 3];
    if w == 0 || h == 0 {
        return out;
    }
    // 天体カメラの画像は 2000 万画素を超えるので、行を分けて並行に埋める。
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(h);
    let rows_per = h.div_ceil(threads);
    std::thread::scope(|scope| {
        for (i, chunk) in out.chunks_mut(rows_per * w * 3).enumerate() {
            scope.spawn(move || fill_rows(src, w, h, pattern, i * rows_per, chunk));
        }
    });
    out
}

fn fill_rows(src: &[f32], w: usize, h: usize, pattern: Pattern, first: usize, out: &mut [f32]) {
    for (r, row) in out.chunks_exact_mut(w * 3).enumerate() {
        let y = first + r;
        for x in 0..w {
            let mut sum = [0.0f32; 3];
            let mut n = [0u32; 3];
            for dy in -1..=1isize {
                for dx in -1..=1isize {
                    let (sx, sy) = (x as isize + dx, y as isize + dy);
                    if sx < 0 || sy < 0 || sx >= w as isize || sy >= h as isize {
                        continue;
                    }
                    let (sx, sy) = (sx as usize, sy as usize);
                    let c = pattern.color_at(sx, sy) as usize;
                    // 自分の色は自分の値だけを使う。周りで平均すると細かな星がぼやけるため。
                    if c == pattern.color_at(x, y) as usize && (dx, dy) != (0, 0) {
                        continue;
                    }
                    sum[c] += src[sy * w + sx];
                    n[c] += 1;
                }
            }
            let px = &mut row[x * 3..][..3];
            for c in 0..3 {
                px[c] = if n[c] > 0 { sum[c] / n[c] as f32 } else { 0.0 };
            }
        }
    }
}
