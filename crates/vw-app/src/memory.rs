//! フル解像度の画像をどのスロットに持たせるかの判断。
//!
//! スロットは常に縮小画像（長辺 [`THUMB_SIDE`]）を持ち、フル解像度の画像は
//! 縮小画像では足りない大きさで表示しているスロットのぶんだけ持つ。フル解像度の画像の
//! 合計が上限を超えないよう、しばらく使っていないものから捨て、必要になったら読み直す。
//! 数百枚を並べたときに、すべてをフル解像度で持つとメモリが足りなくなるため。

/// 縮小画像の長辺の画素数。
pub const THUMB_SIDE: u32 = 512;

/// スロット 1 つの、フル解像度の画像についての状態。
#[derive(Clone, Debug, Default)]
pub struct FullState {
    pub id: u64,
    /// このフレームでフル解像度が必要な大きさで表示した。
    pub wanted: bool,
    /// 持っているフル解像度の画像のバイト数。
    pub resident: Option<usize>,
    /// 読み込み中のフル解像度の画像の見込みバイト数。
    pub pending: Option<usize>,
    /// 最後に必要とされたフレーム番号。
    pub last_used: u64,
    /// 読み込んだときの見込みバイト数。
    pub estimate: usize,
}

/// このフレームで行うこと。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// フル解像度の画像を捨てるスロット。
    pub evict: Vec<u64>,
    /// フル解像度の画像を読み込み始めるスロット。
    pub request: Vec<u64>,
}

fn used(slots: &[FullState]) -> usize {
    slots
        .iter()
        .map(|s| s.resident.unwrap_or(0) + s.pending.unwrap_or(0))
        .sum()
}

/// 捨ててよい画像（持っているが今は必要ない）の、使っていない順。
fn evictable(slots: &[FullState]) -> Vec<&FullState> {
    let mut v: Vec<&FullState> = slots
        .iter()
        .filter(|s| s.resident.is_some() && !s.wanted)
        .collect();
    v.sort_by_key(|s| s.last_used);
    v
}

/// 上限 `limit` バイトに収まるよう、捨てるものと読み込むものを決める。
///
/// 必要なスロットの画像は捨てない。必要なスロットだけで上限に届くときは、
/// それ以上読み込まずに縮小画像のまま表示する。必要なもの同士で捨て合うと、
/// 毎フレーム読み込みと破棄を繰り返してしまうため。
pub fn plan(slots: &[FullState], limit: usize) -> Plan {
    let mut out = Plan::default();
    let mut used = used(slots);
    let candidates = evictable(slots);
    let mut spare: usize = candidates.iter().map(|s| s.resident.unwrap_or(0)).sum();
    let mut candidates = candidates.into_iter();
    let mut evict_one = |used: &mut usize, spare: &mut usize, out: &mut Plan| {
        let s = candidates
            .next()
            .expect("caller checked that spare covers it");
        let bytes = s.resident.unwrap_or(0);
        *used -= bytes;
        *spare -= bytes;
        out.evict.push(s.id);
    };
    while used > limit && spare > 0 {
        evict_one(&mut used, &mut spare, &mut out);
    }
    for s in slots
        .iter()
        .filter(|s| s.wanted && s.resident.is_none() && s.pending.is_none())
    {
        if used + s.estimate > limit + spare {
            continue;
        }
        while used + s.estimate > limit {
            evict_one(&mut used, &mut spare, &mut out);
        }
        used += s.estimate;
        out.request.push(s.id);
    }
    out
}

/// 読み込み終わった `bytes` バイトの画像を、必要でない画像を捨てれば上限内で持てるか。
pub fn fits(slots: &[FullState], bytes: usize, limit: usize) -> bool {
    let spare: usize = evictable(slots)
        .iter()
        .map(|s| s.resident.unwrap_or(0))
        .sum();
    used(slots) + bytes <= limit + spare
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(id: u64) -> FullState {
        FullState {
            id,
            estimate: 100,
            ..Default::default()
        }
    }

    fn resident(id: u64, bytes: usize, last_used: u64, wanted: bool) -> FullState {
        FullState {
            id,
            wanted,
            resident: Some(bytes),
            last_used,
            ..slot(id)
        }
    }

    fn wanted(id: u64) -> FullState {
        FullState {
            wanted: true,
            ..slot(id)
        }
    }

    #[test]
    fn wanted_slot_is_loaded_when_there_is_room() {
        let p = plan(&[wanted(1)], 1000);
        assert_eq!(p.request, [1]);
        assert!(p.evict.is_empty());
    }

    #[test]
    fn unused_images_are_kept_while_under_the_limit() {
        let p = plan(&[resident(1, 300, 0, false), wanted(2)], 1000);
        assert_eq!(
            p,
            Plan {
                evict: vec![],
                request: vec![2]
            }
        );
    }

    #[test]
    fn least_recently_used_image_is_dropped_to_make_room() {
        let slots = [
            resident(1, 400, 5, false),
            resident(2, 400, 1, false),
            wanted(3),
        ];
        let p = plan(&slots, 850);
        assert_eq!(p.evict, [2]);
        assert_eq!(p.request, [3]);
    }

    #[test]
    fn images_on_screen_are_never_dropped() {
        let slots = [resident(1, 900, 0, true), wanted(2)];
        let p = plan(&slots, 950);
        assert_eq!(p, Plan::default(), "slot 2 stays on its thumbnail");
    }

    #[test]
    fn nothing_is_dropped_when_dropping_would_not_make_enough_room() {
        let slots = [
            resident(1, 50, 0, false),
            resident(2, 880, 0, true),
            wanted(3),
        ];
        assert_eq!(plan(&slots, 950), Plan::default());
    }

    #[test]
    fn pending_loads_count_toward_the_limit() {
        let loading = FullState {
            pending: Some(900),
            ..slot(1)
        };
        assert!(plan(&[loading, wanted(2)], 950).request.is_empty());
    }

    #[test]
    fn lowering_the_limit_drops_unused_images_oldest_first() {
        let slots = [
            resident(1, 400, 3, false),
            resident(2, 400, 1, false),
            resident(3, 400, 2, false),
        ];
        assert_eq!(plan(&slots, 500).evict, [2, 3]);
    }

    #[test]
    fn loaded_image_fits_if_unused_ones_can_be_dropped() {
        let slots = [resident(1, 600, 0, false), resident(2, 300, 0, true)];
        assert!(fits(&slots, 600, 1000));
        assert!(!fits(&slots, 800, 1000));
    }
}
