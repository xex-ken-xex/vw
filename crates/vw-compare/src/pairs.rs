//! 似た画像の組を見つける。名前、撮影日時とカメラ、見た目の指紋（縮小画像の差分ハッシュ）の
//! どれかでつながった画像を 1 つの組にする。
//!
//! 画像を読み直さずに済むよう、指紋はスロットに出した縮小画像から作る。

/// 組を探す 1 枚。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
}

/// 指紋がこれ以下しか違わなければ、見た目が似ているとみなす（64 ビットのうち）。
pub const SIMILAR_BITS: u32 = 6;
/// 組の大きさの上限。見た目の近さだけでこれより大きくつながったものは、連写や
/// 動画のコマのような「続き」とみなし、見た目ではつながないことにする。
pub const MAX_GROUP: usize = 9;

/// 似た画像の組（`items` の添字、2 枚以上）。組どうしは重ならない。並びは最初の画像の順。
pub fn groups(items: &[Item]) -> Vec<Vec<usize>> {
    let mut by_name = Union::new(items.len());
    for i in 0..items.len() {
        for j in i + 1..items.len() {
            if same_name(&items[i], &items[j]) || same_shot(&items[i], &items[j]) {
                by_name.join(i, j);
            }
        }
    }
    let mut all = by_name.clone();
    for i in 0..items.len() {
        for j in i + 1..items.len() {
            if looks_alike(&items[i], &items[j]) {
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

/// 撮影日時が秒まで同じで、カメラが食い違わない。日時の書き方（`2026:10:26` と
/// `2026-10-26`）は数字だけで比べる。機種は RAW と JPEG で「NIKON Z 9」と「Z 9」のように
/// 書き方が違うことがあるので、片方がもう片方を含めば同じとみなす。
fn same_shot(a: &Item, b: &Item) -> bool {
    let digits = |s: &str| s.chars().filter(char::is_ascii_digit).collect::<String>();
    let (Some(ta), Some(tb)) = (&a.taken, &b.taken) else {
        return false;
    };
    let (ta, tb) = (digits(ta), digits(tb));
    // 年月日と時分秒がそろっていないものは、別の写真を同じとみなしかねないので使わない。
    if ta.len() < 14 || ta != tb {
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

fn looks_alike(a: &Item, b: &Item) -> bool {
    match (a.fingerprint, b.fingerprint) {
        (Some(x), Some(y)) => (x ^ y).count_ones() <= SIMILAR_BITS,
        _ => false,
    }
}

/// 画像の指紋（差分ハッシュ）。9 × 8 の灰色に縮め、横に隣り合う画素の明るさの大小を
/// 64 ビットにする。明るさやコントラスト、大きさ、JPEG の圧縮が違っても変わりにくい。
pub fn fingerprint(width: usize, height: usize, rgba: &[u8]) -> Option<u64> {
    const W: usize = 9;
    const H: usize = 8;
    if width < W || height < H || rgba.len() < width * height * 4 {
        return None;
    }
    let mut cells = [0f64; W * H];
    for (cy, row) in cells.as_chunks_mut::<W>().0.iter_mut().enumerate() {
        let (y0, y1) = (cy * height / H, (cy + 1) * height / H);
        for (cx, cell) in row.iter_mut().enumerate() {
            let (x0, x1) = (cx * width / W, (cx + 1) * width / W);
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
    let mut bits = 0u64;
    for row in cells.as_chunks::<W>().0 {
        for x in 0..W - 1 {
            bits = bits << 1 | u64::from(row[x] < row[x + 1]);
        }
    }
    Some(bits)
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
        assert_eq!(groups(&items), [vec![0, 1]]);
    }

    #[test]
    fn the_same_name_in_another_folder_is_not_a_pair() {
        assert!(groups(&[item("a", "x"), item("b", "x")]).is_empty());
    }

    #[test]
    fn the_same_time_and_camera_make_a_pair_even_when_renamed() {
        let shot = |stem: &str, taken: &str, camera: &str| Item {
            taken: Some(taken.into()),
            camera: Some(camera.into()),
            ..item("a", stem)
        };
        let items = [
            shot("DSC_0001", "2026:10:26 12:00:01", "NIKON Z 9"),
            shot("edited", "2026-10-26 12:00:01", "Z 9"),
            shot("other", "2026-10-26 12:00:01", "X100V"),
            shot("later", "2026-10-26 12:00:02", "Z 9"),
        ];
        assert_eq!(groups(&items), [vec![0, 1]]);
    }

    #[test]
    fn a_date_without_the_time_is_not_enough() {
        let day = |stem: &str| Item {
            taken: Some("2026-10-26".into()),
            ..item("a", stem)
        };
        assert!(groups(&[day("a"), day("b")]).is_empty());
    }

    #[test]
    fn images_that_look_alike_are_a_pair() {
        let with = |stem: &str, f: u64| Item {
            fingerprint: Some(f),
            ..item("a", stem)
        };
        let items = [with("a", 0xF0F0), with("b", 0xF0F3), with("c", !0xF0F0)];
        assert_eq!(groups(&items), [vec![0, 1]]);
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
        assert_eq!(groups(&items), [vec![0, 12]]);
    }

    fn gradient(w: usize, h: usize, gain: f64) -> Vec<u8> {
        (0..w * h)
            .flat_map(|i| {
                let (x, y) = ((i % w) as f64, (i / w) as f64);
                let v = ((x / w as f64 * 200.0 + (y * 0.05).sin() * 40.0) * gain).min(255.0) as u8;
                [v, v, v, 255]
            })
            .collect()
    }

    #[test]
    fn the_fingerprint_survives_resizing_and_brightness() {
        let a = fingerprint(90, 80, &gradient(90, 80, 1.0)).unwrap();
        let b = fingerprint(180, 160, &gradient(180, 160, 1.2)).unwrap();
        assert!((a ^ b).count_ones() <= SIMILAR_BITS, "{a:064b}\n{b:064b}");
        let flipped: Vec<u8> = gradient(90, 80, 1.0)
            .as_chunks::<4>()
            .0
            .as_chunks::<90>()
            .0
            .iter()
            .flat_map(|row| row.iter().rev().flatten().copied().collect::<Vec<u8>>())
            .collect();
        let c = fingerprint(90, 80, &flipped).unwrap();
        assert!((a ^ c).count_ones() > 20);
    }

    #[test]
    fn a_tiny_image_has_no_fingerprint() {
        assert_eq!(fingerprint(4, 4, &[0; 64]), None);
    }
}
