//! メタデータ欄に出すグループの並び。隠した項目と、自分で作ったグループの設定を当てる。
//!
//! 項目は `グループ/項目` の英語の名前（[`Row::id`]）で見分ける。画面の言語を切り替えても
//! 隠した項目や集めた項目が変わらないようにするため。

use crate::settings::{MetadataSettings, SearchGroup};

/// 欄の 1 行。
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// `グループ/項目`。自分で作ったグループの行も、元のグループの名前を持つ。
    pub id: String,
    /// 画面に出す項目名。
    pub key: String,
    pub value: String,
    /// AI タグの行なら、そのタグ。
    pub tag: Option<String>,
    /// 隠した項目を一時的に出しているとき、隠した項目なら true。
    pub hidden: bool,
}

impl Row {
    pub fn new(group: &str, key: &str, shown_key: String, value: String) -> Self {
        Self {
            id: format!("{group}/{key}"),
            key: shown_key,
            value,
            tag: None,
            hidden: false,
        }
    }
}

/// 欄の 1 グループ。
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    /// 英語の名前。自分で作ったグループはその名前。
    pub id: String,
    pub title: String,
    pub rows: Vec<Row>,
    pub hidden: bool,
    /// 自分で作ったグループ。
    pub custom: bool,
}

impl Group {
    pub fn new(id: &str, title: String, rows: Vec<Row>) -> Self {
        Self {
            id: id.to_owned(),
            title,
            rows,
            hidden: false,
            custom: false,
        }
    }
}

/// 画像から読んだグループ（`found`）に設定を当てる。自分で作ったグループを先頭に置き、
/// 隠したものは `show_hidden` なら印を付けて残し、そうでなければ外す。
/// 行がなくなったグループは出さない（今の画像にない項目だけを集めたグループも同じ）。
pub fn arrange(found: Vec<Group>, settings: &MetadataSettings, show_hidden: bool) -> Vec<Group> {
    let custom = settings.groups.iter().map(|g| Group {
        id: g.name.clone(),
        title: g.name.clone(),
        rows: g
            .keys
            .iter()
            .filter_map(|k| found.iter().flat_map(|f| &f.rows).find(|r| &r.id == k))
            .cloned()
            .collect(),
        hidden: false,
        custom: true,
    });
    let groups: Vec<Group> = custom.collect();
    groups
        .into_iter()
        .chain(found)
        .filter_map(|mut g| {
            g.hidden = settings.is_hidden(&g.id);
            if !g.custom {
                for r in &mut g.rows {
                    r.hidden = settings.is_hidden(&r.id);
                }
            }
            if !show_hidden {
                if g.hidden {
                    return None;
                }
                g.rows.retain(|r| !r.hidden);
            }
            (!g.rows.is_empty()).then_some(g)
        })
        .collect()
}

/// 隠した項目があるか。欄の見出しに「隠した項目も出す」を出すかどうかに使う。
pub fn any_hidden(settings: &MetadataSettings) -> bool {
    !settings.hidden.is_empty()
}

/// 欄の右クリックで選ぶ、表示の設定の変更。
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// 項目（`グループ/項目`）かグループを隠す。
    Hide(String),
    Show(String),
    /// 項目を、名前のグループに足す。なければ作る。
    AddToGroup(String, String),
    /// 名前を聞いてから [`Edit::AddToGroup`] にする。
    NewGroup(String),
    RemoveFromGroup(String, String),
    DeleteGroup(String),
}

/// `edit` を設定に当てる。変わったら true。
pub fn apply(settings: &mut MetadataSettings, edit: Edit) -> bool {
    match edit {
        Edit::Hide(id) => {
            if settings.is_hidden(&id) {
                return false;
            }
            settings.hidden.push(id);
            true
        }
        Edit::Show(id) => {
            let before = settings.hidden.len();
            settings.hidden.retain(|h| *h != id);
            settings.hidden.len() != before
        }
        Edit::AddToGroup(row, name) => {
            match settings.groups.iter_mut().find(|g| g.name == name) {
                Some(g) if g.keys.contains(&row) => return false,
                Some(g) => g.keys.push(row),
                None => settings.groups.push(SearchGroup {
                    name,
                    keys: vec![row],
                }),
            }
            true
        }
        Edit::NewGroup(_) => false,
        Edit::RemoveFromGroup(row, name) => {
            let Some(i) = settings.groups.iter().position(|g| g.name == name) else {
                return false;
            };
            let before = settings.groups[i].keys.len();
            settings.groups[i].keys.retain(|k| *k != row);
            let changed = settings.groups[i].keys.len() != before;
            // キーのないグループは設定として読めない（[`SearchGroup::problem`]）ので、グループごと消す。
            if settings.groups[i].keys.is_empty() {
                settings.groups.remove(i);
            }
            changed
        }
        Edit::DeleteGroup(name) => {
            let before = settings.groups.len();
            settings.groups.retain(|g| g.name != name);
            settings.groups.len() != before
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found() -> Vec<Group> {
        let row = |g: &str, k: &str, v: &str| Row::new(g, k, k.into(), v.into());
        vec![
            Group::new(
                "File",
                "ファイル".into(),
                vec![
                    row("File", "Path", "/a.png"),
                    row("File", "Size", "10 bytes"),
                ],
            ),
            Group::new(
                "EXIF",
                "EXIF".into(),
                vec![
                    row("EXIF", "Model", "X100"),
                    row("EXIF", "MakerNote", "..."),
                ],
            ),
        ]
    }

    fn ids(groups: &[Group]) -> Vec<(String, Vec<String>)> {
        groups
            .iter()
            .map(|g| (g.id.clone(), g.rows.iter().map(|r| r.id.clone()).collect()))
            .collect()
    }

    fn settings(hidden: &[&str], groups: &[(&str, &[&str])]) -> MetadataSettings {
        MetadataSettings {
            hidden: hidden.iter().map(|&h| h.into()).collect(),
            groups: groups
                .iter()
                .map(|(name, keys)| SearchGroup {
                    name: (*name).into(),
                    keys: keys.iter().map(|&k| k.into()).collect(),
                })
                .collect(),
            ..MetadataSettings::default()
        }
    }

    #[test]
    fn hidden_rows_and_groups_are_left_out() {
        let s = settings(&["File/Size", "EXIF"], &[]);
        let out = arrange(found(), &s, false);
        assert_eq!(ids(&out), vec![("File".into(), vec!["File/Path".into()])]);
    }

    #[test]
    fn showing_hidden_keeps_them_marked() {
        let s = settings(&["File/Size", "EXIF"], &[]);
        let out = arrange(found(), &s, true);
        assert_eq!(out.len(), 2);
        assert!(!out[0].rows[0].hidden && out[0].rows[1].hidden);
        assert!(out[1].hidden);
    }

    #[test]
    fn a_group_whose_rows_are_all_hidden_is_left_out() {
        let s = settings(&["EXIF/Model", "EXIF/MakerNote"], &[]);
        let out = arrange(found(), &s, false);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn custom_groups_come_first_in_their_own_order() {
        let s = settings(&[], &[("Mine", &["EXIF/Model", "File/Path"])]);
        let out = arrange(found(), &s, false);
        assert_eq!(
            ids(&out)[0],
            ("Mine".into(), vec!["EXIF/Model".into(), "File/Path".into()])
        );
        assert!(out[0].custom);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn custom_group_keeps_rows_hidden_in_their_own_group() {
        let s = settings(&["EXIF/Model"], &[("Mine", &["EXIF/Model"])]);
        let out = arrange(found(), &s, false);
        assert_eq!(ids(&out)[0], ("Mine".into(), vec!["EXIF/Model".into()]));
        assert_eq!(ids(&out)[2], ("EXIF".into(), vec!["EXIF/MakerNote".into()]));
    }

    #[test]
    fn custom_group_without_rows_in_this_image_is_left_out() {
        let s = settings(&[], &[("DICOM", &["DICOM/Modality"])]);
        let out = arrange(found(), &s, false);
        assert!(out.iter().all(|g| !g.custom));
    }

    #[test]
    fn hide_and_show_toggle_the_hidden_list() {
        let mut s = MetadataSettings::default();
        assert!(apply(&mut s, Edit::Hide("File/Size".into())));
        assert!(!apply(&mut s, Edit::Hide("File/Size".into())));
        assert_eq!(s.hidden, vec!["File/Size".to_owned()]);
        assert!(apply(&mut s, Edit::Show("File/Size".into())));
        assert!(s.hidden.is_empty());
    }

    #[test]
    fn adding_to_a_new_name_creates_the_group() {
        let mut s = MetadataSettings::default();
        assert!(apply(
            &mut s,
            Edit::AddToGroup("EXIF/Model".into(), "Mine".into())
        ));
        assert!(apply(
            &mut s,
            Edit::AddToGroup("File/Path".into(), "Mine".into())
        ));
        assert!(!apply(
            &mut s,
            Edit::AddToGroup("File/Path".into(), "Mine".into())
        ));
        assert_eq!(s.groups.len(), 1);
        assert_eq!(s.groups[0].keys, vec!["EXIF/Model", "File/Path"]);
    }

    #[test]
    fn removing_the_last_item_deletes_the_group() {
        let mut s = settings(&[], &[("Mine", &["EXIF/Model"])]);
        assert!(apply(
            &mut s,
            Edit::RemoveFromGroup("EXIF/Model".into(), "Mine".into())
        ));
        assert!(s.groups.is_empty());
    }

    #[test]
    fn delete_group_removes_only_that_group() {
        let mut s = settings(&[], &[("A", &["File/Path"]), ("B", &["File/Size"])]);
        assert!(apply(&mut s, Edit::DeleteGroup("A".into())));
        assert_eq!(s.groups.len(), 1);
        assert_eq!(s.groups[0].name, "B");
    }
}
