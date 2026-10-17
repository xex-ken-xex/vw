//! パネル（フォルダ・メタデータ・ログの欄）の部品。

/// パネルの見出し。そのパネルのボタンは、同じ行の右に詰めて置く。
///
/// 見出しの下に線や余白を足さないのは、狭い画面で中身の行を 1 行でも多く見せるため。
/// 大文字にもしない: 日本語では変わらず、英語では字が詰まって読みにくくなる。
pub fn header(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let color = ui.visuals().text_color();
    // 絞り込みの条件などで長くなっても、欄を押し広げないよう省略する。
    ui.add(
        egui::Label::new(egui::RichText::new(text).size(HEADER_SIZE).color(color))
            .truncate()
            .selectable(false),
    )
}

/// 見出しの字の大きさ。中身の字（13px 前後）より少しだけ小さくして、見出しだと分かるようにする。
const HEADER_SIZE: f32 = 12.0;
