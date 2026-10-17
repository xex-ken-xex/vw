//! 欄に収まらない文字列の出し方。

/// `text` が `fits` に収まらなければ、頭を「…」にして末尾をできるだけ残す。
///
/// 連番のファイルやパスは違いが末尾に出るので、egui の省略（末尾を切る）は使わない。
pub fn tail_that_fits(text: &str, fits: impl Fn(&str) -> bool) -> String {
    if fits(text) {
        return text.to_owned();
    }
    let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let tail = |k: usize| format!("…{}", &text[starts[k]..]);
    let (mut lo, mut hi) = (0, starts.len().saturating_sub(1));
    while lo < hi {
        let mid = usize::midpoint(lo, hi);
        if fits(&tail(mid)) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    tail(lo)
}

/// 残りの幅に収まるよう頭を省いた 1 行の字。乗せると `hover` を、なければ省いたときだけ全体を出す。
pub fn tail_label(ui: &mut egui::Ui, text: &str, hover: Option<&str>) -> egui::Response {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let width = ui.available_width();
    let shown = tail_that_fits(text, |s| {
        ui.painter()
            .layout_no_wrap(s.to_owned(), font.clone(), egui::Color32::PLACEHOLDER)
            .size()
            .x
            <= width
    });
    let hover = hover.or((shown != text).then_some(text));
    let response = ui.add(egui::Label::new(shown).extend());
    match hover {
        Some(h) => response.on_hover_text(h),
        None => response,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_names_keep_their_end_behind_an_ellipsis() {
        let fits = |s: &str| s.chars().count() <= 8;
        assert_eq!(tail_that_fits("a.png", fits), "a.png");
        assert_eq!(tail_that_fits("scan-0001-0042.ima", fits), "…042.ima");
        assert_eq!(
            tail_that_fits("写真の名前がとても長い.jpg", fits),
            "…も長い.jpg"
        );
    }
}
