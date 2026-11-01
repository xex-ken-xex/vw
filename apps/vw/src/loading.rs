//! 読み込みの判断。何を頼むか、届いた結果をどのスロットへ届けるか、届いた画像をどう置くかを、
//! テクスチャや描画を持たない値だけで決める。実行するのは [`crate::app`]。
//!
//! 判断をここに分けてあるのは、読み込みの経路（loader → スロット → 描画）が不具合の集まる
//! 場所で、しかもアプリ全体を組み立てないと試せなかったため。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::Vec2;

use crate::frames;
use crate::memory;
use crate::viewer::Sequence;

/// 今のマス目で並べたとき、1 枠が縮小画像より大きいか。`area` は画像欄の大きさ（物理ピクセル）。
///
/// 大きければ、最初の読み込みでフル解像度も一緒に作る。縮小画像だけ先に読んで
/// 後からフル解像度を読み直すと、1 枚を大きく見る普段の使い方でデコードが 2 回になるため。
/// 画像の大きさは読むまで分からないので、枠の大きさだけで決める。
///
/// 最初のフレームの前（枠の大きさがまだ分からない）は縮小画像だけにする。起動時の
/// フォルダの復元や引数で大きなフォルダに入ると、見えない枠まで全部フル解像度で頼み、
/// 数千枚で何 GB も使っていたため。画面が出てから、要る枠だけが後から頼む。
pub fn cell_needs_full(area: Option<Vec2>, cols: usize, rows: usize) -> bool {
    let Some(area) = area else {
        return false;
    };
    let cell = (area.x / cols.max(1) as f32).max(area.y / rows.max(1) as f32);
    cell > memory::THUMB_SIDE as f32
}

/// 読み込み結果を待っているスロット。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Waiting {
    pub id: u64,
    /// 最後に出した読み込み要求の番号。
    pub request: u64,
    /// 読み込み中のフル解像度（現像）の要求番号。
    pub full_request: Option<u64>,
}

/// 届いた結果の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// 画像を開いたときの最初の読み込み（縮小画像、あればフル解像度も）。
    First,
    /// あとから頼んだフル解像度か現像。
    Full,
}

/// スロット `slot` への要求 `request` の結果を届ける先。スロットが閉じられた、別の画像に
/// 切り替えられた、古い要求だったときは `None` で、結果は捨てる。
pub fn deliver(slots: &[Waiting], slot: u64, request: u64) -> Option<(usize, Delivery)> {
    let index = slots.iter().position(|s| s.id == slot)?;
    let s = &slots[index];
    if s.request == request {
        Some((index, Delivery::First))
    } else if s.full_request == Some(request) {
        Some((index, Delivery::Full))
    } else {
        None
    }
}

/// 届いた画像のうち、置き方を決めるのに要るもの。
#[derive(Debug, Clone, Default)]
pub struct Incoming {
    /// ファイルに入っているフレームの数。フレームを指すパスで読んだときは 1。
    pub frames: usize,
    /// ファイルが勧める送りの速さ。
    pub fps: Option<f32>,
    /// フレームごとの表示時間（ミリ秒）。
    pub delays: Option<Arc<[u32]>>,
    /// 並びの中で送っても倍率と位置を保つ形式（DICOM）。設定が切でも保つ。
    pub keeps_view: bool,
}

/// 最初の読み込みが届いたときの置き方。
#[derive(Debug)]
pub struct Arrival {
    /// 表示する画像のパス。マルチフレームのファイルなら最初のフレームを指すパス。
    pub path: PathBuf,
    /// 1 枚のスロットで開いたマルチフレームのファイルを、フレームの並びにする。
    pub sequence: Option<Sequence>,
    /// 倍率と位置を全体表示に戻す。同じ画像の読み直しと、並びの中の送りでは戻さない。
    pub reset_view: bool,
}

/// スロットに `path` の最初の読み込みが届いた。`shown` は今出している画像、`sequence` は
/// スロットの並び、`keep_view` は設定「送っても表示を保つ」。
pub fn arrival(
    shown: Option<&Path>,
    path: PathBuf,
    sequence: Option<&Sequence>,
    incoming: &Incoming,
    keep_view: bool,
) -> Arrival {
    // 同じ画像の読み直し（上書きされたとき）は、拡大している場所を見続けられるよう倍率と位置を保つ。
    let reloaded = shown == Some(path.as_path());
    // 1 枚のスロットで開いたマルチフレームのファイルは、フレームの並びのスロットにする。
    // フォルダスロットの中では並べ替えない。フォルダの中を送れなくなるため。
    let multiframe = incoming.frames > 1 && frames::split(&path).is_none();
    let (path, new_sequence) = if multiframe && sequence.is_none() {
        let mut seq = Sequence::frames(path, incoming.frames, incoming.fps);
        seq.delays = incoming.delays.clone();
        (seq.paths[0].clone(), Some(seq))
    } else {
        (path, None)
    };
    let stepped = keeps_view(
        shown,
        &path,
        sequence.or(new_sequence.as_ref()),
        keep_view || incoming.keeps_view,
    );
    Arrival {
        path,
        sequence: new_sequence,
        reset_view: !reloaded && !stepped,
    }
}

/// 複数枚のスロットの中で `previous` から `next` に送ったときに、倍率と位置を引き継ぐか。
/// スライスやフレームを送って同じ場所を見比べるため。別の並びに入れ替えたときは引き継がない。
pub fn keeps_view(
    previous: Option<&Path>,
    next: &Path,
    sequence: Option<&Sequence>,
    enabled: bool,
) -> bool {
    let (Some(previous), Some(seq)) = (previous, sequence) else {
        return false;
    };
    enabled && seq.paths.iter().any(|p| p == previous) && seq.paths.iter().any(|p| p == next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::vec2;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn nothing_needs_full_resolution_before_the_first_frame() {
        assert!(!cell_needs_full(None, 1, 1));
    }

    #[test]
    fn one_large_cell_needs_full_resolution_and_a_grid_of_small_cells_does_not() {
        let area = Some(vec2(1920.0, 1080.0));
        assert!(cell_needs_full(area, 1, 1));
        assert!(cell_needs_full(area, 2, 1));
        assert!(!cell_needs_full(area, 4, 4));
    }

    #[test]
    fn the_longer_side_of_the_cell_decides() {
        // 横に 4 つ並べても、縦が縮小画像より大きければフル解像度が要る。
        assert!(cell_needs_full(Some(vec2(2000.0, 600.0)), 4, 1));
        assert!(!cell_needs_full(Some(vec2(2000.0, 500.0)), 4, 1));
    }

    #[test]
    fn an_empty_grid_does_not_divide_by_zero() {
        assert!(cell_needs_full(Some(vec2(600.0, 600.0)), 0, 0));
    }

    fn waiting() -> Vec<Waiting> {
        vec![
            Waiting {
                id: 10,
                request: 5,
                full_request: Some(7),
            },
            Waiting {
                id: 11,
                request: 6,
                full_request: None,
            },
        ]
    }

    #[test]
    fn a_result_goes_to_the_slot_that_asked_for_it() {
        assert_eq!(deliver(&waiting(), 10, 5), Some((0, Delivery::First)));
        assert_eq!(deliver(&waiting(), 11, 6), Some((1, Delivery::First)));
    }

    #[test]
    fn a_full_resolution_result_is_told_apart_from_the_first_load() {
        assert_eq!(deliver(&waiting(), 10, 7), Some((0, Delivery::Full)));
    }

    #[test]
    fn results_of_old_requests_and_closed_slots_are_dropped() {
        // 前後へ素早く送ったあとに遅れて届いた前の画像。
        assert_eq!(deliver(&waiting(), 10, 4), None);
        // 閉じたスロットへの結果。
        assert_eq!(deliver(&waiting(), 12, 5), None);
        // 番号は合うがスロットが違う。
        assert_eq!(deliver(&waiting(), 11, 5), None);
    }

    fn single() -> Incoming {
        Incoming {
            frames: 1,
            ..Incoming::default()
        }
    }

    #[test]
    fn a_new_image_resets_the_view() {
        let a = arrival(Some(&p("a.png")), p("b.png"), None, &single(), true);
        assert_eq!(a.path, p("b.png"));
        assert!(a.sequence.is_none());
        assert!(a.reset_view);
    }

    #[test]
    fn reloading_the_same_image_keeps_the_view() {
        let a = arrival(Some(&p("a.png")), p("a.png"), None, &single(), false);
        assert!(!a.reset_view);
    }

    #[test]
    fn stepping_inside_a_sequence_keeps_the_view_only_when_enabled() {
        let seq = Sequence::new(p("dir"), p("dir/a.png"));
        let mut seq = seq;
        seq.paths = vec![p("dir/a.png"), p("dir/b.png")];
        let step = |enabled| {
            arrival(
                Some(&p("dir/a.png")),
                p("dir/b.png"),
                Some(&seq),
                &single(),
                enabled,
            )
        };
        assert!(!step(true).reset_view);
        assert!(step(false).reset_view);
    }

    #[test]
    fn dicom_keeps_the_view_even_when_the_setting_is_off() {
        let mut seq = Sequence::new(p("dir"), p("dir/a"));
        seq.paths = vec![p("dir/a"), p("dir/b")];
        let dicom = Incoming {
            keeps_view: true,
            ..single()
        };
        let a = arrival(Some(&p("dir/a")), p("dir/b"), Some(&seq), &dicom, false);
        assert!(!a.reset_view);
    }

    #[test]
    fn stepping_to_an_image_outside_the_sequence_resets_the_view() {
        let mut seq = Sequence::new(p("dir"), p("dir/a.png"));
        seq.paths = vec![p("dir/a.png"), p("dir/b.png")];
        assert!(
            arrival(
                Some(&p("dir/a.png")),
                p("other.png"),
                Some(&seq),
                &single(),
                true
            )
            .reset_view
        );
        assert!(arrival(None, p("dir/b.png"), Some(&seq), &single(), true).reset_view);
    }

    #[test]
    fn a_multiframe_file_in_a_plain_slot_becomes_a_sequence_of_its_frames() {
        let incoming = Incoming {
            frames: 3,
            fps: Some(10.0),
            delays: Some(Arc::from([100, 100, 200])),
            keeps_view: false,
        };
        let a = arrival(None, p("anim.gif"), None, &incoming, true);
        let seq = a.sequence.expect("a sequence of frames");
        assert_eq!(seq.paths.len(), 3);
        assert_eq!(a.path, seq.paths[0]);
        assert_eq!(seq.fps, Some(10.0));
        assert_eq!(seq.delays.as_deref(), Some(&[100, 100, 200][..]));
        assert!(a.reset_view);
    }

    #[test]
    fn a_multiframe_file_inside_a_folder_slot_stays_one_image_of_the_folder() {
        let mut seq = Sequence::new(p("dir"), p("dir/anim.gif"));
        seq.paths = vec![p("dir/anim.gif"), p("dir/b.png")];
        let incoming = Incoming {
            frames: 3,
            ..single()
        };
        let a = arrival(None, p("dir/anim.gif"), Some(&seq), &incoming, true);
        assert!(a.sequence.is_none());
        assert_eq!(a.path, p("dir/anim.gif"));
    }

    #[test]
    fn a_single_frame_of_a_multiframe_file_is_not_split_again() {
        let frame = frames::frame_paths(&p("anim.gif"), 3)[1].clone();
        let incoming = Incoming {
            frames: 3,
            ..single()
        };
        let a = arrival(None, frame.clone(), None, &incoming, true);
        assert!(a.sequence.is_none());
        assert_eq!(a.path, frame);
    }
}
