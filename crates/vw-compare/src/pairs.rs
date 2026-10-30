//! 似た画像の組を見つける。名前、撮影日時とカメラ（同じ時刻か、連写の間隔の内）、見た目の
//! 指紋（縮小画像の知覚ハッシュ）のどれかでつながった画像を 1 つの組にする。組の中は鮮鋭度で
//! 順位を付け、いちばんくっきりした 1 枚を残す候補にする。
//!
//! 画像を読み直さずに済むよう、指紋と鮮鋭度はスロットに出した縮小画像から作る。

/// 組を探す 1 枚。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Item {
    /// 入っているフォルダ（名前で組にするのは同じフォルダの中だけ）。
    pub folder: String,
    /// 拡張子を除いたファイル名。
    pub stem: String,
    /// 撮影日時（EXIF の DateTimeOriginal など）。
    pub taken: Option<String>,
    /// カメラの機種。
    pub camera: Option<String>,
    pub fingerprint: Option<u64>,
    /// 鮮鋭度（[`sharpness`]）。組の中の順位に使う。
    pub sharpness: Option<f64>,
    /// 画素数（幅 × 高さ）。鮮鋭度が同じくらいのときに、大きい方を先にする。
    pub pixels: u64,
}

/// 組にする決まり。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rules {
    /// 指紋がこれ以下しか違わなければ、見た目が似ているとみなす（64 ビットのうち）。
    pub similar_bits: u32,
    /// 同じカメラで、撮影日時がこの秒数以内に続けて撮ったものを連写として 1 つの組にする。
    /// 0 なら、時刻が秒まで同じものだけ。
    pub burst_seconds: u32,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            similar_bits: SIMILAR_BITS,
            burst_seconds: 2,
        }
    }
}

/// [`Rules::similar_bits`] の既定。知覚ハッシュの、同じ写真の縮小・再圧縮・明るさの違いが
/// 収まり、構図の違う写真がまず入らない幅。
pub const SIMILAR_BITS: u32 = 10;
/// 組の大きさの上限。見た目の近さだけでこれより大きくつながったものは、連写や
/// 動画のコマのような「続き」とみなし、見た目ではつながないことにする。
pub const MAX_GROUP: usize = 9;

/// 似た画像の組（`items` の添字、2 枚以上）。組どうしは重ならない。並びは最初の画像の順。
pub fn groups(items: &[Item], rules: Rules) -> Vec<Vec<usize>> {
    let mut by_name = Union::new(items.len());
    for i in 0..items.len() {
        for j in i + 1..items.len() {
            if same_name(&items[i], &items[j])
                || same_shot(&items[i], &items[j], rules.burst_seconds)
            {
                by_name.join(i, j);
            }
        }
    }
    let mut all = by_name.clone();
    for i in 0..items.len() {
        for j in i + 1..items.len() {
            if looks_alike(&items[i], &items[j], rules.similar_bits) {
                all.join(i, j);
            }
        }
    }
    let mut out = Vec::new();
    for group in all.groups() {
        if group.len() <= MAX_GROUP {
            out.push(group);
            continue;
        }
        let members: Vec<usize> = group;
        out.extend(
            by_name
                .groups()
                .into_iter()
                .filter(|g| g.iter().all(|i| members.contains(i))),
        );
    }
    out.retain(|g| g.len() >= 2);
    out.sort_by_key(|g| g[0]);
    out
}

/// 同じフォルダで、拡張子を除いた名前が同じ（大文字・小文字は区別しない）。
fn same_name(a: &Item, b: &Item) -> bool {
    a.folder == b.folder && !a.stem.is_empty() && a.stem.to_lowercase() == b.stem.to_lowercase()
}

/// 撮影日時の差が `burst_seconds` 秒以内で、カメラが食い違わない。日時の書き方（`2026:10:26` と
/// `2026-10-26`）は数字だけで読む。機種は RAW と JPEG で「NIKON Z 9」と「Z 9」のように
/// 書き方が違うことがあるので、片方がもう片方を含めば同じとみなす。
fn same_shot(a: &Item, b: &Item, burst_seconds: u32) -> bool {
    let (Some(ta), Some(tb)) = (
        a.taken.as_deref().and_then(seconds),
        b.taken.as_deref().and_then(seconds),
    ) else {
        return false;
    };
    if ta.abs_diff(tb) > u64::from(burst_seconds) {
        return false;
    }
    match (&a.camera, &b.camera) {
        (Some(ca), Some(cb)) => {
            let (ca, cb) = (ca.trim().to_lowercase(), cb.trim().to_lowercase());
            ca.contains(&cb) || cb.contains(&ca)
        }
        _ => true,
    }
}

/// 撮影日時を、ある日からの秒数にする。年月日と時分秒がそろっていないものは、別の写真を
/// 同じとみなしかねないので読まない。
fn seconds(taken: &str) -> Option<u64> {
    let digits: Vec<u64> = taken
        .chars()
        .filter_map(|c| c.to_digit(10).map(u64::from))
        .collect();
    if digits.len() < 14 {
        return None;
    }
    let number = |range: std::ops::Range<usize>| digits[range].iter().fold(0, |n, d| n * 10 + d);
    let (year, month, day) = (number(0..4), number(4..6), number(6..8));
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year as i64, month as i64, day as i64);
    Some(days as u64 * 86_400 + number(8..10) * 3600 + number(10..12) * 60 + number(12..14))
}

/// 西暦 0 年 3 月 1 日からの日数（H. Hinnant の days_from_civil）。月の長さとうるう年を
/// 表で持たずに済む。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe
}

fn looks_alike(a: &Item, b: &Item, similar_bits: u32) -> bool {
    match (a.fingerprint, b.fingerprint) {
        (Some(x), Some(y)) => (x ^ y).count_ones() <= similar_bits,
        _ => false,
    }
}

/// 鮮鋭度がいちばん高い画像のこの割合以上なら、同じくらいくっきりしているとみなす。
const SHARP_ENOUGH: f64 = 0.9;

/// 組の画像を、残す候補の順に並べる。鮮鋭度がいちばん高い画像と同じくらいのものを先に、
/// その中は画素数の多い順に置く。残りは鮮鋭度の高い順、鮮鋭度の分からない画像は最後に元の順で。
///
/// 鮮鋭度だけで並べないのは、縮めて圧縮し直した複製の方が、縮小画像の上では元の写真より
/// わずかにくっきり出ることがあり、元の写真を差し置いて残す候補になってしまうため。
pub fn ranked(items: &[Item], group: &[usize]) -> Vec<usize> {
    let sharpness = |i: usize| items[i].sharpness.unwrap_or(f64::NEG_INFINITY);
    let best = group
        .iter()
        .map(|&i| sharpness(i))
        .fold(f64::NEG_INFINITY, f64::max);
    let sharp_enough = |i: usize| best > 0.0 && sharpness(i) >= best * SHARP_ENOUGH;
    let mut out = group.to_vec();
    out.sort_by(|&a, &b| {
        sharp_enough(b)
            .cmp(&sharp_enough(a))
            .then_with(|| {
                if sharp_enough(a) && sharp_enough(b) {
                    items[b].pixels.cmp(&items[a].pixels)
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then_with(|| sharpness(b).total_cmp(&sharpness(a)))
    });
    out
}

/// 画像の指紋（知覚ハッシュ）。32 × 32 の灰色に縮めて 2 次元の離散コサイン変換をかけ、
/// 低い周波数の 8 × 8 の係数がその中央値より大きいかを 64 ビットにする。明るさやコントラスト、
/// 大きさ、JPEG の圧縮が違っても変わりにくい。
///
/// 前は隣り合う画素の明るさの大小（差分ハッシュ）を使っていた。差分ハッシュは平らな空や壁の
/// ように隣どうしがほぼ同じ明るさの所で、わずかなノイズでビットが入れ替わり、同じ写真を
/// 見落としやすいので、周波数の成分で比べる方にした。
pub fn fingerprint(width: usize, height: usize, rgba: &[u8]) -> Option<u64> {
    const N: usize = 32;
    const K: usize = 8;
    let gray = shrink_gray(width, height, rgba, N)?;
    let cos: Vec<f64> = (0..K * N)
        .map(|i| {
            let (u, x) = (i / N, i % N);
            (std::f64::consts::PI * (2 * x + 1) as f64 * u as f64 / (2 * N) as f64).cos()
        })
        .collect();
    // 行ごとに低い 8 つの周波数だけ求め、次に列の向きに同じことをする。
    let mut rows = [0f64; N * K];
    for y in 0..N {
        for u in 0..K {
            rows[y * K + u] = (0..N).map(|x| gray[y * N + x] * cos[u * N + x]).sum();
        }
    }
    let mut coeffs = [0f64; K * K];
    for v in 0..K {
        for u in 0..K {
            coeffs[v * K + u] = (0..N).map(|y| rows[y * K + u] * cos[v * N + y]).sum();
        }
    }
    // 直流（全体の明るさ）は中央値を決めるのに入れない。明るさだけで中央値が動かないように。
    let mut sorted: Vec<f64> = coeffs[1..].to_vec();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    Some(
        coeffs
            .iter()
            .fold(0u64, |bits, &c| bits << 1 | u64::from(c > median)),
    )
}

/// 鮮鋭度。縮小画像を 4 × 4 のマスに分け、マスごとのラプラシアンの分散のいちばん大きい値。
/// ピントの合った所がどこか一部でもあれば高くなる。
///
/// 画像全体の分散にしないのは、背景をぼかした写真では、ピントの合った被写体の小さな面積が
/// 全体のぼけに薄められ、被写体がぶれた 1 枚と順位が入れ替わりやすいため。
pub fn sharpness(width: usize, height: usize, rgba: &[u8]) -> Option<f64> {
    const GRID: usize = 4;
    if width < GRID * 3 || height < GRID * 3 || rgba.len() < width * height * 4 {
        return None;
    }
    let luma: Vec<f32> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .take(width * height)
        .map(|p| {
            (0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2])) / 255.0
        })
        .collect();
    let mut best: Option<f64> = None;
    for gy in 0..GRID {
        for gx in 0..GRID {
            let (x0, x1) = (gx * width / GRID, (gx + 1) * width / GRID);
            let (y0, y1) = (gy * height / GRID, (gy + 1) * height / GRID);
            let tile: Vec<f32> = (y0..y1)
                .flat_map(|y| luma[y * width + x0..y * width + x1].iter().copied())
                .collect();
            let image = vw_quality::Image::new((x1 - x0) as u32, (y1 - y0) as u32, &tile);
            if let Some(v) = vw_quality::laplacian_variance(&image) {
                best = Some(best.map_or(v, |b| b.max(v)));
            }
        }
    }
    best
}

/// `side` × `side` の灰色（0〜255）に、面積の平均で縮める。
fn shrink_gray(width: usize, height: usize, rgba: &[u8], side: usize) -> Option<Vec<f64>> {
    if width < side || height < side || rgba.len() < width * height * 4 {
        return None;
    }
    let mut cells = vec![0f64; side * side];
    for (cy, row) in cells.chunks_mut(side).enumerate() {
        let (y0, y1) = (cy * height / side, (cy + 1) * height / side);
        for (cx, cell) in row.iter_mut().enumerate() {
            let (x0, x1) = (cx * width / side, (cx + 1) * width / side);
            let mut sum = 0.0;
            for y in y0..y1 {
                for p in rgba[(y * width + x0) * 4..(y * width + x1) * 4]
                    .as_chunks::<4>()
                    .0
                {
                    sum +=
                        0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2]);
                }
            }
            *cell = sum / ((y1 - y0) * (x1 - x0)) as f64;
        }
    }
    Some(cells)
}

/// つながった画像をまとめる（素集合）。
#[derive(Clone)]
struct Union {
    parent: Vec<usize>,
}

impl Union {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn root(&mut self, mut i: usize) -> usize {
        while self.parent[i] != i {
            self.parent[i] = self.parent[self.parent[i]];
            i = self.parent[i];
        }
        i
    }

    fn join(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.root(a), self.root(b));
        if ra != rb {
            self.parent[ra.max(rb)] = ra.min(rb);
        }
    }

    fn groups(&self) -> Vec<Vec<usize>> {
        let mut u = self.clone();
        let mut by_root: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
        for i in 0..u.parent.len() {
            let r = u.root(i);
            by_root.entry(r).or_default().push(i);
        }
        by_root.into_values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(folder: &str, stem: &str) -> Item {
        Item {
            folder: folder.into(),
            stem: stem.into(),
            ..Item::default()
        }
    }

    #[test]
    fn a_raw_and_a_jpeg_with_the_same_name_are_a_pair() {
        let items = [
            item("a", "DSC_0001"),
            item("a", "dsc_0001"),
            item("a", "DSC_0002"),
        ];
        assert_eq!(groups(&items, Rules::default()), [vec![0, 1]]);
    }

    #[test]
    fn the_same_name_in_another_folder_is_not_a_pair() {
        assert!(groups(&[item("a", "x"), item("b", "x")], Rules::default()).is_empty());
    }

    fn shot(stem: &str, taken: &str, camera: &str) -> Item {
        Item {
            taken: Some(taken.into()),
            camera: Some(camera.into()),
            ..item("a", stem)
        }
    }

    const NO_BURST: Rules = Rules {
        similar_bits: SIMILAR_BITS,
        burst_seconds: 0,
    };

    #[test]
    fn the_same_time_and_camera_make_a_pair_even_when_renamed() {
        let items = [
            shot("DSC_0001", "2026:10:26 12:00:01", "NIKON Z 9"),
            shot("edited", "2026-10-26 12:00:01", "Z 9"),
            shot("other", "2026-10-26 12:00:01", "X100V"),
            shot("later", "2026-10-26 12:00:02", "Z 9"),
        ];
        assert_eq!(groups(&items, NO_BURST), [vec![0, 1]]);
    }

    #[test]
    fn shots_in_quick_succession_are_one_burst() {
        let items = [
            shot("1", "2026:10:26 12:00:01", "Z 9"),
            shot("2", "2026:10:26 12:00:02", "Z 9"),
            shot("3", "2026:10:26 12:00:04", "Z 9"),
            shot("apart", "2026:10:26 12:00:09", "Z 9"),
            shot("other camera", "2026:10:26 12:00:03", "X100V"),
        ];
        assert_eq!(groups(&items, Rules::default()), [vec![0, 1, 2]]);
    }

    #[test]
    fn a_burst_can_cross_midnight_and_month_ends() {
        let items = [
            shot("1", "2026:10:31 23:59:59", "Z 9"),
            shot("2", "2026:11:01 00:00:01", "Z 9"),
        ];
        assert_eq!(groups(&items, Rules::default()), [vec![0, 1]]);
    }

    #[test]
    fn a_zero_burst_gap_joins_only_the_same_second() {
        let items = [
            shot("1", "2026:10:26 12:00:01", "Z 9"),
            shot("2", "2026:10:26 12:00:02", "Z 9"),
        ];
        assert!(groups(&items, NO_BURST).is_empty());
    }

    #[test]
    fn the_sharpest_image_leads_its_group() {
        let with = |stem: &str, s: Option<f64>| Item {
            sharpness: s,
            ..item("a", stem)
        };
        let items = [
            with("a", Some(1.0)),
            with("b", None),
            with("c", Some(5.0)),
            with("d", Some(2.0)),
        ];
        assert_eq!(ranked(&items, &[0, 1, 2, 3]), [2, 3, 0, 1]);
    }

    #[test]
    fn among_equally_sharp_images_the_largest_leads() {
        let with = |stem: &str, s: f64, pixels: u64| Item {
            sharpness: Some(s),
            pixels,
            ..item("a", stem)
        };
        let items = [
            with("small copy", 1.00, 800 * 600),
            with("original", 0.95, 1600 * 1200),
            with("blurred original", 0.20, 1600 * 1200),
        ];
        assert_eq!(ranked(&items, &[0, 1, 2]), [1, 0, 2]);
    }

    #[test]
    fn a_date_without_the_time_is_not_enough() {
        let day = |stem: &str| Item {
            taken: Some("2026-10-26".into()),
            ..item("a", stem)
        };
        assert!(groups(&[day("a"), day("b")], Rules::default()).is_empty());
    }

    #[test]
    fn images_that_look_alike_are_a_pair() {
        let with = |stem: &str, f: u64| Item {
            fingerprint: Some(f),
            ..item("a", stem)
        };
        let items = [with("a", 0xF0F0), with("b", 0xF0F3), with("c", !0xF0F0)];
        assert_eq!(groups(&items, Rules::default()), [vec![0, 1]]);
        let strict = Rules {
            similar_bits: 1,
            ..Rules::default()
        };
        assert!(groups(&items, strict).is_empty());
    }

    #[test]
    fn a_long_run_of_alike_images_is_not_one_group() {
        let mut items: Vec<Item> = (0..12)
            .map(|i| Item {
                fingerprint: Some(0),
                ..item("a", &format!("frame{i}"))
            })
            .collect();
        items.push(Item {
            fingerprint: Some(0),
            ..item("a", "frame0")
        });
        // 似た 13 枚は 1 つにはしないが、名前で組になる 2 枚は残す。
        assert_eq!(groups(&items, Rules::default()), [vec![0, 12]]);
    }

    /// 決まった種から作る、0〜`amp` の揺らぎ。
    fn noise(n: usize, seed: u64, amp: f64) -> Vec<f64> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x % 1000) as f64 / 1000.0 * amp
            })
            .collect()
    }

    /// 上半分が空（揺らぎ入り）、下に山の影と建物の明るい窓が並ぶ景色。
    fn scene(w: usize, h: usize, seed: u64, gain: f64) -> Vec<u8> {
        let n = noise(w * h, seed, 8.0);
        (0..w * h)
            .flat_map(|i| {
                let (x, y) = ((i % w) as f64 / w as f64, (i / w) as f64 / h as f64);
                let ridge = 0.55 + 0.12 * (x * 9.0).sin() + 0.05 * (x * 23.0).cos();
                let v = if y < ridge {
                    150.0 + y * 60.0
                } else if (x * 10.0).fract() < 0.4 && (y * 8.0).fract() < 0.5 {
                    200.0
                } else {
                    40.0
                };
                let v = ((v + n[i]) * gain).min(255.0) as u8;
                [v, v, v, 255]
            })
            .collect()
    }

    #[test]
    fn the_fingerprint_survives_resizing_noise_and_exposure() {
        let a = fingerprint(256, 192, &scene(256, 192, 1, 1.0)).unwrap();
        let b = fingerprint(320, 240, &scene(320, 240, 2, 1.15)).unwrap();
        assert!((a ^ b).count_ones() <= SIMILAR_BITS, "{a:064b}\n{b:064b}");
    }

    #[test]
    fn a_mirrored_scene_has_a_different_fingerprint() {
        let a = fingerprint(256, 192, &scene(256, 192, 1, 1.0)).unwrap();
        let mirrored: Vec<u8> = scene(256, 192, 1, 1.0)
            .as_chunks::<4>()
            .0
            .chunks(256)
            .flat_map(|row| row.iter().rev().flatten().copied().collect::<Vec<u8>>())
            .collect();
        let c = fingerprint(256, 192, &mirrored).unwrap();
        assert!(
            (a ^ c).count_ones() > 2 * SIMILAR_BITS,
            "{a:064b}\n{c:064b}"
        );
    }

    #[test]
    fn a_blurred_image_is_less_sharp() {
        let (w, h) = (128, 128);
        let sharp: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let v = if (i % w / 8 + i / w / 8) % 2 == 0 {
                    30
                } else {
                    220
                };
                [v, v, v, 255]
            })
            .collect();
        let blurred: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                let mut sum = 0u32;
                for dy in 0..5 {
                    for dx in 0..5 {
                        let (sx, sy) = ((x + dx).min(w - 1), (y + dy).min(h - 1));
                        sum += u32::from(sharp[(sy * w + sx) * 4]);
                    }
                }
                let v = (sum / 25) as u8;
                [v, v, v, 255]
            })
            .collect();
        let a = sharpness(w, h, &sharp).unwrap();
        let b = sharpness(w, h, &blurred).unwrap();
        assert!(a > b * 2.0, "{a} {b}");
    }

    #[test]
    fn a_small_sharp_subject_on_a_blurred_background_counts_as_sharp() {
        let (w, h) = (128, 128);
        let subject = |sharp: bool| -> Vec<u8> {
            (0..w * h)
                .flat_map(|i| {
                    let (x, y) = (i % w, i / w);
                    let v = if x < 32 && y < 32 && sharp && (x / 2 + y / 2) % 2 == 0 {
                        250
                    } else {
                        128
                    };
                    [v, v, v, 255]
                })
                .collect()
        };
        assert!(
            sharpness(w, h, &subject(true)).unwrap() > sharpness(w, h, &subject(false)).unwrap()
        );
    }

    #[test]
    fn a_tiny_image_has_no_fingerprint() {
        assert_eq!(fingerprint(4, 4, &[0; 64]), None);
    }
}
