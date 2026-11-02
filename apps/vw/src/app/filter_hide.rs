//! 絞り込み（フラグ・タグ）の間、合わないスロットを並びから外しておく。
//!
//! 外すのは、フォルダのスロットと、連なり（GIF のコマ、DICOM のシリーズなど）のうち合う画像の
//! ないスロット。空のまま場所を取ると、絞り込みが効いていないように見えるため。
//!
//! スロットに「隠れている」印を付けて描かないだけにしないのは、並べ方・ページ・← → の移動が
//! どれも `slots` の番号で数えていて、印を見る所をすべてに足すことになるため。外したスロットは
//! 前にあったスロットの id と一緒に取っておき、絞り込みを外したら同じ所に戻す。

use super::*;

/// 外しておいたスロット。
#[derive(Default)]
pub(super) struct HiddenSlots {
    /// 外したスロットと、そのとき直前にあったスロットの id（先頭なら `None`）。
    slots: Vec<(Option<u64>, Slot)>,
    /// どれも合わず並びが空になったときに置いた空のスロット。戻すときに消す。
    placeholder: Option<u64>,
}

impl HiddenSlots {
    pub(super) fn clear(&mut self) {
        self.slots.clear();
        self.placeholder = None;
    }
}

/// 絞り込みの間、並びから外すスロットか。
fn hidden_while_filtering(slot: &Slot) -> bool {
    slot.folder().is_some()
        || (slot.sequence.is_some()
            && slot.path.is_none()
            && !slot.loading
            && matches!(slot.content, Content::Empty))
}

/// `slots` から `hide` に当たるものを抜き出す。抜いたものには、直前に残ったスロットの id を添える。
fn take_out(slots: &mut Vec<Slot>, hide: impl Fn(&Slot) -> bool) -> Vec<(Option<u64>, Slot)> {
    let mut kept = Vec::with_capacity(slots.len());
    let mut taken = Vec::new();
    for slot in std::mem::take(slots) {
        if hide(&slot) {
            taken.push((kept.last().map(|s: &Slot| s.id), slot));
        } else {
            kept.push(slot);
        }
    }
    *slots = kept;
    taken
}

/// 抜き出したスロットを、直前にあったスロットの後ろへ戻す。同じスロットの後ろにあったものは
/// 元の順に並べる。直前のスロットがもうなければ先頭に戻す。
fn put_back(slots: &mut Vec<Slot>, hidden: Vec<(Option<u64>, Slot)>) {
    let mut last_after: std::collections::HashMap<Option<u64>, u64> =
        std::collections::HashMap::new();
    for (after, slot) in hidden {
        let anchor = last_after.get(&after).copied().or(after);
        let at = anchor
            .and_then(|id| slots.iter().position(|s| s.id == id))
            .map_or(0, |i| i + 1);
        let at = if anchor.is_some() && at == 0 {
            // 直前のスロットが閉じられていた。先頭の、同じ種類のスロットの後ろへ。
            slots
                .iter()
                .rposition(|s| s.folder().is_some())
                .map_or(0, |i| i + 1)
        } else {
            at
        };
        last_after.insert(after, slot.id);
        slots.insert(at, slot);
    }
}

impl VwApp {
    /// 外しておいたスロットをすべて戻す。今のフォルダにもうないフォルダや、同じフォルダの
    /// スロットがもう並んでいるもの（外している間に並べ直された）は戻さない。
    pub(super) fn show_hidden_slots(&mut self) {
        let hidden = std::mem::take(&mut self.hidden_slots);
        if hidden.slots.is_empty() {
            return;
        }
        let active_id = self.slots.get(self.active).map(|s| s.id);
        if let Some(id) = hidden.placeholder
            && self.slots.len() == 1
            && self.slots[0].id == id
            && is_blank(&self.slots[0])
        {
            self.slots.clear();
        }
        let back: Vec<(Option<u64>, Slot)> = hidden
            .slots
            .into_iter()
            .filter(|(_, s)| match s.folder() {
                Some(folder) => {
                    folder.exists() && !self.slots.iter().any(|o| o.folder() == Some(folder))
                }
                None => true,
            })
            .collect();
        put_back(&mut self.slots, back);
        if self.slots.is_empty() {
            self.add_slot();
        }
        self.active = active_id
            .and_then(|id| self.slots.iter().position(|s| s.id == id))
            .unwrap_or(0);
    }

    /// 絞り込みの間は、合わないスロットを並びから外す。絞り込みがなければ戻す。
    /// フォルダの読み込みが進んで増えたスロットにも効かせるため、毎フレーム呼ぶ。
    pub(super) fn keep_filter_hiding(&mut self) {
        if self.filter == Filter::All {
            self.show_hidden_slots();
            return;
        }
        if !self.slots.iter().any(hidden_while_filtering) {
            return;
        }
        let active_id = self.slots.get(self.active).map(|s| s.id);
        let taken = take_out(&mut self.slots, hidden_while_filtering);
        if self
            .maximized
            .is_some_and(|id| taken.iter().any(|(_, s)| s.id == id))
        {
            self.maximized = None;
        }
        self.hidden_slots.slots.extend(taken);
        if self.slots.is_empty() {
            self.add_slot();
            self.hidden_slots.placeholder = self.slots.last().map(|s| s.id);
        }
        self.active = active_id
            .and_then(|id| self.slots.iter().position(|s| s.id == id))
            .unwrap_or(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewer::{Sequence, SequenceKind};

    fn folder(id: u64, name: &str) -> Slot {
        let mut s = Slot::new(id);
        s.set_subfolder(PathBuf::from(name));
        s
    }

    fn image(id: u64) -> Slot {
        let mut s = Slot::new(id);
        s.path = Some(PathBuf::from(format!("{id}.png")));
        s
    }

    fn ids(slots: &[Slot]) -> Vec<u64> {
        slots.iter().map(|s| s.id).collect()
    }

    #[test]
    fn folder_slots_are_taken_out_and_put_back_where_they_were() {
        let mut slots = vec![folder(1, "a"), folder(2, "b"), image(3), image(4)];
        let taken = take_out(&mut slots, hidden_while_filtering);
        assert_eq!(ids(&slots), [3, 4]);
        put_back(&mut slots, taken);
        assert_eq!(ids(&slots), [1, 2, 3, 4]);
    }

    #[test]
    fn slots_hidden_between_images_return_between_them() {
        let mut slots = vec![image(1), folder(2, "a"), image(3), folder(4, "b"), image(5)];
        let taken = take_out(&mut slots, hidden_while_filtering);
        assert_eq!(ids(&slots), [1, 3, 5]);
        put_back(&mut slots, taken);
        assert_eq!(ids(&slots), [1, 2, 3, 4, 5]);
    }

    #[test]
    fn a_slot_whose_neighbour_was_closed_returns_after_the_folders() {
        let mut slots = vec![folder(1, "a"), image(2), folder(3, "b"), image(4)];
        let taken = take_out(&mut slots, hidden_while_filtering);
        slots.retain(|s| s.id != 2);
        put_back(&mut slots, taken);
        assert_eq!(ids(&slots), [1, 3, 4]);
    }

    fn sequence() -> Sequence {
        Sequence {
            folder: PathBuf::from("f"),
            kind: SequenceKind::Folder,
            paths: vec![PathBuf::from("f/1.png")],
            index: 0,
            fps: None,
            delays: None,
            title: None,
        }
    }

    #[test]
    fn an_empty_sequence_slot_is_hidden_but_a_loading_one_is_not() {
        let mut empty = Slot::new(1);
        empty.sequence = Some(sequence());
        let mut loading = Slot::new(2);
        loading.sequence = Some(sequence());
        loading.loading = true;
        assert!(hidden_while_filtering(&empty));
        assert!(!hidden_while_filtering(&loading));
        assert!(!hidden_while_filtering(&image(3)));
    }
}
