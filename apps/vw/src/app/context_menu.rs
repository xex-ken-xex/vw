//! 右クリックメニュー。操作の表（[`SLOT_SECTIONS`], [`FOLDER_SECTIONS`]）から描くので、
//! スロットとツリーで同じ操作が同じ名前・同じキー表示で出る。「今使えるか」は
//! [`availability`] の 1 か所で決め、キーからの実行（[`VwApp::run_on`]）も同じ判断を通る。
//!
//! 送り先・色の表・人物のように設定で中身が増える小メニューは、表には [`Entry`] として
//! 置き場所だけを書き、描くときにその場で中身を作る。

use std::path::{Path, PathBuf};

use eframe::egui;

use super::VwApp;
use crate::faces::FaceMark;
use crate::i18n::{tr, tr1};
use crate::keymap::{Action, KeyCombo, Keymap};
use crate::settings::Language;
use crate::viewer::{FaceAction, Slot};

/// メニューの 1 項目。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Act(Action),
    /// 右クリックした顔の枠の「登録 ▸」「新しい人物として登録...」「人物を外す」。
    Faces,
    /// 「色の表 ▸」。
    ColorMap,
    /// 「送る ▸」。
    Send,
}

use Entry::Act;

/// 画像のスロットの右クリック。節ごとに区切り線を入れる。
pub const SLOT_SECTIONS: &[&[Entry]] = &[
    &[Entry::Faces],
    &[
        Act(Action::CopyMeasurement),
        Act(Action::DeleteProbePoint),
        Act(Action::ToggleLineProfile),
        Act(Action::ToggleRoiGraph),
    ],
    &[
        Act(Action::FitImage),
        Act(Action::CopyImage),
        Act(Action::ComposeColors),
        Act(Action::AlignAndCompare),
    ],
    &[Act(Action::SetQualityReference), Act(Action::ReadQrCodes)],
    &[
        Act(Action::AiTagImage),
        Act(Action::FindFacesImage),
        Act(Action::RemoveAiTagsImage),
    ],
    &[
        Act(Action::DevelopRaw),
        Act(Action::ToggleCalibration),
        Entry::ColorMap,
        Act(Action::TogglePixelPipeline),
        Entry::Send,
    ],
];

/// ツリーのフォルダ・ZIP の右クリック。
pub const FOLDER_SECTIONS: &[&[Entry]] = &[
    &[
        Act(Action::CopyFolderPath),
        Act(Action::OpenInFileManager),
        Act(Action::CloseZip),
        Act(Action::AiTagFolder),
        Act(Action::FindFacesFolder),
        Act(Action::RemoveAiTagsFolder),
    ],
    &[
        Act(Action::RegisterDataset),
        Act(Action::EditDataset),
        Act(Action::UpdateDataset),
    ],
    &[Entry::Send],
];

/// ツリーの項目の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderKind {
    Plain,
    /// 展開した ZIP のルート。中は書き込めない。
    MountedRoot,
    /// まだ展開していない ZIP ファイル。
    Zip,
}

/// 操作の対象（描くときと「使えるか」を決めるとき）。
pub enum Target<'a> {
    Slot {
        index: usize,
        slot: &'a Slot,
        /// 表示中の画像で見つけた顔の枠。
        faces: &'a [FaceMark],
    },
    Folder {
        path: &'a Path,
        kind: FolderKind,
    },
}

/// 操作の対象（実行するとき）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Where {
    /// キーから。アクティブなスロットと cd しているフォルダ。
    Active,
    Slot(usize),
    Folder(PathBuf, FolderKind),
}

impl Target<'_> {
    fn location(&self) -> Where {
        match self {
            Target::Slot { index, .. } => Where::Slot(*index),
            Target::Folder { path, kind } => Where::Folder(path.to_path_buf(), *kind),
        }
    }

    fn slot(&self) -> Option<&Slot> {
        match self {
            Target::Slot { slot, .. } => Some(slot),
            Target::Folder { .. } => None,
        }
    }

    fn folder(&self) -> Option<(&Path, FolderKind)> {
        match self {
            Target::Folder { path, kind } => Some((path, *kind)),
            Target::Slot { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Yes,
    /// メニューに出さない（意味がない）。
    Hidden,
    /// 灰色で出す。理由（英語の原文）をマウスを乗せたときに出す。
    Disabled(&'static str),
}

/// 操作 `action` を `target` に今使えるか。右クリックとキーの両方がここを通る。
pub fn availability(action: Action, target: &Target, ai_ready: bool) -> Availability {
    use Availability::*;
    let shown_if = |yes: bool| if yes { Yes } else { Hidden };
    let ai = || {
        if ai_ready {
            Yes
        } else {
            Disabled(crate::ai_models::NOT_READY)
        }
    };
    let slot = target.slot();
    let folder = target.folder();
    let in_archive = folder.is_some_and(|(_, kind)| kind != FolderKind::Plain);
    match action {
        Action::CopyMeasurement | Action::DeleteProbePoint => {
            shown_if(slot.is_some_and(|s| measurement_text(s).is_some()))
        }
        Action::ToggleLineProfile => match slot {
            Some(s) => shown_if(!s.shown_lines().is_empty()),
            None => Yes,
        },
        Action::ToggleRoiGraph => match slot {
            Some(s) => shown_if(
                s.roi_of(crate::roi::Kind::Target).is_some()
                    || s.roi_of(crate::roi::Kind::Background).is_some(),
            ),
            None => Yes,
        },
        Action::ComposeColors => shown_if(slot.is_some_and(composable)),
        Action::DevelopRaw => shown_if(slot.is_some_and(developable)),
        Action::ToggleCalibration => shown_if(slot.and_then(calibration_on).is_some()),
        Action::AiTagImage | Action::FindFacesImage => ai(),
        // 展開した ZIP の中には書けないので、AI タグとデータセットの操作は出さない。
        Action::AiTagFolder | Action::FindFacesFolder if in_archive => Hidden,
        Action::AiTagFolder | Action::FindFacesFolder => ai(),
        Action::RemoveAiTagsFolder
        | Action::RegisterDataset
        | Action::EditDataset
        | Action::UpdateDataset
            if in_archive =>
        {
            Hidden
        }
        Action::RegisterDataset => match folder {
            Some((path, _)) => shown_if(!crate::catalog::exists_in(path)),
            None => Yes,
        },
        Action::EditDataset | Action::UpdateDataset => match folder {
            Some((path, _)) => shown_if(crate::catalog::exists_in(path)),
            None => Yes,
        },
        Action::CloseZip => {
            shown_if(folder.is_some_and(|(_, kind)| kind == FolderKind::MountedRoot))
        }
        _ => Yes,
    }
}

/// 選んでいる十字の値か、ROI の統計か、計測線の値。十字・ROI・線の順（消すときも同じ順）。
pub fn measurement_text(slot: &Slot) -> Option<String> {
    slot.selected_mark_text()
        .or_else(|| slot.selected_roi_text())
        .or_else(|| slot.selected_line_text())
}

fn composable(slot: &Slot) -> bool {
    slot.shown()
        .is_some_and(|s| s.info.compose.is_some() || s.info.color == vw_core::ColorModel::Gray)
}

fn developable(slot: &Slot) -> bool {
    slot.developable()
}

fn calibration_on(slot: &Slot) -> Option<bool> {
    slot.calibration_on()
}

/// 右クリックに出す名前。設定画面の名前（[`Action::title`]）より短く、対象が分かっているので
/// 「この画像に」などを省く。
pub fn context_title(action: Action, target: &Target) -> &'static str {
    let mark = target
        .slot()
        .is_some_and(|s| s.selected_mark_text().is_some());
    let roi = target
        .slot()
        .is_some_and(|s| s.selected_roi_text().is_some());
    match action {
        Action::CopyImage | Action::CopyFolderPath => "Copy",
        Action::AiTagImage => "Add AI tags",
        Action::FindFacesImage => "Find faces",
        Action::RemoveAiTagsImage => "Remove AI tags",
        Action::CopyMeasurement if mark => "Copy point value",
        Action::CopyMeasurement if roi => "Copy ROI statistics",
        Action::CopyMeasurement => "Copy line values",
        Action::DeleteProbePoint if mark => "Delete point",
        Action::DeleteProbePoint if roi => "Delete ROI",
        Action::DeleteProbePoint => "Delete line",
        Action::ToggleLineProfile => "Line profile",
        Action::ToggleRoiGraph => "ROI graph",
        Action::TogglePixelPipeline => "Pixel pipeline...",
        Action::OpenInFileManager => "Open in file manager",
        Action::CloseZip => "Close ZIP",
        Action::RegisterDataset => "Register as dataset...",
        Action::EditDataset => "Edit dataset...",
        Action::UpdateDataset => "Update dataset",
        other => other.title(),
    }
}

/// マウスを乗せたときの説明（英語の原文）。
fn hint(action: Action) -> Option<&'static str> {
    match action {
        Action::ComposeColors => Some("Put two or three gray images on red, green and blue."),
        Action::AlignAndCompare => {
            Some("Line up similar images with this one and measure how they differ.")
        }
        Action::SetQualityReference => {
            Some("PSNR and SSIM in the analysis window compare the active image with this one.")
        }
        _ => None,
    }
}

/// メニューを描くのに要る、アプリの状態。
pub struct MenuContext<'a> {
    pub lang: Language,
    pub keys: &'a Keymap,
    /// AI のモデルが全部揃っているか。揃うまで AI の項目は押せない。
    pub ai_ready: bool,
    /// 「送る ▸」に並べるアプリと送信先の名前。
    pub apps: &'a [crate::settings::SendApp],
    pub destinations: &'a [String],
    pub color_rules: &'a crate::colormap::Rules,
    /// 「登録 ▸」に並べる人物。
    pub people: &'a [String],
}

/// メニューで選んだこと。
#[derive(Debug, Clone, PartialEq)]
pub enum Picked {
    Run(Action, Where),
    /// 顔の枠の操作（表示中の画像に対して）。
    Face(PathBuf, FaceAction),
    Send {
        choice: crate::send_app::Choice,
        from: Where,
    },
    /// スロットの色の表を選んだ。
    ColorPick(usize, crate::colormap::Pick),
}

/// 画像のスロットの右クリックメニュー。画像を出していないスロットには出さない。
pub fn slot_menu(
    response: &egui::Response,
    cx: &MenuContext,
    index: usize,
    slot: &Slot,
    faces: &[FaceMark],
) -> Vec<Picked> {
    let mut picked = Vec::new();
    if slot.shown().is_none() {
        return picked;
    }
    let target = Target::Slot { index, slot, faces };
    vw_ui::menu::context(response, |ui| {
        sections(ui, cx, SLOT_SECTIONS, &target, &mut picked);
    });
    picked
}

/// ツリーのフォルダ・ZIP の右クリックメニュー。
pub fn folder_menu(
    response: &egui::Response,
    cx: &MenuContext,
    path: &Path,
    kind: FolderKind,
) -> Vec<Picked> {
    let mut picked = Vec::new();
    let target = Target::Folder { path, kind };
    vw_ui::menu::context(response, |ui| {
        sections(ui, cx, FOLDER_SECTIONS, &target, &mut picked);
    });
    picked
}

fn sections(
    ui: &mut egui::Ui,
    cx: &MenuContext,
    table: &[&[Entry]],
    target: &Target,
    out: &mut Vec<Picked>,
) {
    let mut drawn_before = false;
    for section in table {
        let mut drawn_here = false;
        for &entry in *section {
            if !entry_shows(entry, target, cx) {
                continue;
            }
            if drawn_before && !drawn_here {
                ui.separator();
            }
            drawn_here = true;
            match entry {
                Act(action) => action_item(ui, cx, action, target, out),
                Entry::Faces => faces_items(ui, cx, target, out),
                Entry::ColorMap => color_map_item(ui, cx, target, out),
                Entry::Send => send_item(ui, cx, target, out),
            }
        }
        drawn_before |= drawn_here;
    }
}

fn entry_shows(entry: Entry, target: &Target, cx: &MenuContext) -> bool {
    match entry {
        Act(action) => availability(action, target, cx.ai_ready) != Availability::Hidden,
        Entry::Faces => selected_face(target).is_some(),
        Entry::ColorMap => target
            .slot()
            .and_then(|s| s.shown())
            .is_some_and(|s| s.info.look.is_some()),
        Entry::Send => true,
    }
}

fn action_item(
    ui: &mut egui::Ui,
    cx: &MenuContext,
    action: Action,
    target: &Target,
    out: &mut Vec<Picked>,
) {
    let lang = cx.lang;
    let checked = match action {
        Action::ToggleCalibration => target.slot().and_then(calibration_on) == Some(true),
        _ => false,
    };
    let shortcut = cx
        .keys
        .keys(action)
        .first()
        .map(KeyCombo::to_string)
        .unwrap_or_default();
    let button = vw_ui::menu::item(checked, tr(lang, context_title(action, target)), shortcut);
    let mut response = match availability(action, target, cx.ai_ready) {
        Availability::Disabled(why) => ui
            .add_enabled(false, button)
            .on_disabled_hover_text(tr(lang, why)),
        _ => ui.add(button),
    };
    if let Some(text) = hint(action) {
        response = response.on_hover_text(tr(lang, text));
    }
    if response.clicked() {
        out.push(Picked::Run(action, target.location()));
        ui.close();
    }
}

/// 右クリックした顔の枠（表示中の画像のものに限る）。
fn selected_face<'a>(target: &'a Target) -> Option<(&'a Path, usize, &'a FaceMark)> {
    let Target::Slot { slot, faces, .. } = target else {
        return None;
    };
    let (path, i) = slot.selected_face()?;
    (Some(path) == slot.shown().map(|s| s.path.as_path()))
        .then(|| faces.get(i).map(|face| (path, i, face)))
        .flatten()
}

fn faces_items(ui: &mut egui::Ui, cx: &MenuContext, target: &Target, out: &mut Vec<Picked>) {
    let lang = cx.lang;
    let Some((path, i, face)) = selected_face(target) else {
        return;
    };
    let title = match &face.person {
        Some(name) => tr1(lang, "Face {}", &format!("{}: {name}", i + 1)),
        None => tr1(lang, "Face {}", &format!("{}", i + 1)),
    };
    ui.weak(title);
    if !cx.people.is_empty() {
        ui.menu_button(tr(lang, "Register as"), |ui| {
            for name in cx.people {
                if ui.button(name).clicked() {
                    out.push(Picked::Face(
                        path.to_path_buf(),
                        FaceAction::Assign(i, name.clone()),
                    ));
                    ui.close();
                }
            }
        });
    }
    if ui.button(tr(lang, "Register as a new person...")).clicked() {
        out.push(Picked::Face(path.to_path_buf(), FaceAction::NewPerson(i)));
        ui.close();
    }
    if face.person.is_some() && ui.button(tr(lang, "Unlink person")).clicked() {
        out.push(Picked::Face(path.to_path_buf(), FaceAction::Clear(i)));
        ui.close();
    }
}

fn color_map_item(ui: &mut egui::Ui, cx: &MenuContext, target: &Target, out: &mut Vec<Picked>) {
    let Target::Slot { index, slot, .. } = target else {
        return;
    };
    let Some(applied) = slot.shown().and_then(|s| s.info.look.as_ref()) else {
        return;
    };
    vw_ui::menu::button(ui, tr(cx.lang, "Color map"), |ui| {
        if let Some(pick) = crate::colormap_ui::pick_menu(ui, cx.lang, applied, cx.color_rules) {
            out.push(Picked::ColorPick(*index, pick));
        }
    });
}

fn send_item(ui: &mut egui::Ui, cx: &MenuContext, target: &Target, out: &mut Vec<Picked>) {
    let offer = match target {
        Target::Slot { slot, .. } => {
            let Some(shown) = slot.shown() else { return };
            // ネットワークの送信先は DICOM だけ。
            let dicom = shown.info.plugin == "DICOM";
            let file = crate::frames::file_of(&shown.path);
            crate::send_app::Offer::for_path(
                cx.apps,
                dicom.then_some(cx.destinations),
                &file,
                false,
            )
        }
        Target::Folder { path, kind } => crate::send_app::Offer::for_path(
            cx.apps,
            Some(cx.destinations),
            path,
            *kind != FolderKind::Zip,
        ),
    };
    if let Some(choice) = crate::send_app::menu(ui, cx.lang, &offer) {
        out.push(Picked::Send {
            choice,
            from: target.location(),
        });
    }
}

impl VwApp {
    /// 操作 `action` を `at` に今使えるか（[`availability`]）。キーからは `Where::Active`。
    pub(super) fn available(&self, action: Action, at: &Where) -> Availability {
        let target = match at {
            Where::Folder(path, kind) => Target::Folder { path, kind: *kind },
            Where::Slot(i) => Target::Slot {
                index: *i,
                slot: &self.slots[*i],
                faces: &[],
            },
            Where::Active => match self.slots.get(self.active) {
                Some(slot) => Target::Slot {
                    index: self.active,
                    slot,
                    faces: &[],
                },
                None => return Availability::Yes,
            },
        };
        availability(action, &target, self.ai_ready())
    }

    /// 右クリックメニューで選んだことを行う。
    pub(super) fn apply_picked(&mut self, ctx: &egui::Context, picked: Picked) {
        match picked {
            Picked::Run(action, at) => self.run_on(ctx, action, at),
            Picked::Face(path, action) => {
                let file = crate::frames::file_of(&path);
                match action {
                    FaceAction::Assign(f, name) => self.assign_face(&file, f, &name),
                    FaceAction::NewPerson(f) => self.people_window.ask_name(file, f),
                    FaceAction::Clear(f) => self.clear_face(&file, f),
                }
            }
            Picked::Send { choice, from } => {
                let targets = match from {
                    Where::Active => self.slot_send_targets(self.active),
                    Where::Slot(i) => self.slot_send_targets(i),
                    Where::Folder(path, _) => vec![path],
                };
                self.send_to(choice, targets);
            }
            Picked::ColorPick(i, pick) => self.set_color_pick(i, pick),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewer::{Content, test_support};

    fn folder(kind: FolderKind) -> Target<'static> {
        Target::Folder {
            path: Path::new("/photos/2026"),
            kind,
        }
    }

    fn slot_with(shown: crate::viewer::Shown) -> Slot {
        let mut slot = Slot::new(1);
        slot.content = Content::Image(shown);
        slot
    }

    #[test]
    fn every_action_in_the_tables_is_a_known_action() {
        for section in SLOT_SECTIONS.iter().chain(FOLDER_SECTIONS) {
            for entry in *section {
                if let Act(action) = entry {
                    assert!(Action::ALL.contains(action), "{action:?}");
                }
            }
        }
    }

    #[test]
    fn ai_items_are_disabled_until_the_models_are_ready_in_both_menus() {
        let slot = slot_with(test_support::gray_image());
        let on_slot = Target::Slot {
            index: 0,
            slot: &slot,
            faces: &[],
        };
        for (action, target) in [
            (Action::AiTagImage, &on_slot),
            (Action::FindFacesImage, &on_slot),
            (Action::AiTagFolder, &folder(FolderKind::Plain)),
            (Action::FindFacesFolder, &folder(FolderKind::Plain)),
        ] {
            assert_eq!(
                availability(action, target, false),
                Availability::Disabled(crate::ai_models::NOT_READY),
                "{action:?}"
            );
            assert_eq!(
                availability(action, target, true),
                Availability::Yes,
                "{action:?}"
            );
        }
    }

    #[test]
    fn nothing_is_written_inside_an_extracted_zip() {
        for kind in [FolderKind::MountedRoot, FolderKind::Zip] {
            for action in [
                Action::AiTagFolder,
                Action::FindFacesFolder,
                Action::RemoveAiTagsFolder,
                Action::RegisterDataset,
                Action::EditDataset,
                Action::UpdateDataset,
            ] {
                assert_eq!(
                    availability(action, &folder(kind), true),
                    Availability::Hidden,
                    "{action:?} {kind:?}"
                );
            }
        }
    }

    #[test]
    fn only_the_root_of_an_extracted_zip_can_be_closed() {
        assert_eq!(
            availability(Action::CloseZip, &folder(FolderKind::MountedRoot), true),
            Availability::Yes
        );
        assert_eq!(
            availability(Action::CloseZip, &folder(FolderKind::Plain), true),
            Availability::Hidden
        );
        assert_eq!(
            availability(Action::CloseZip, &folder(FolderKind::Zip), true),
            Availability::Hidden
        );
    }

    #[test]
    fn a_folder_without_a_catalog_offers_registration_and_not_editing() {
        let target = folder(FolderKind::Plain);
        assert_eq!(
            availability(Action::RegisterDataset, &target, true),
            Availability::Yes
        );
        assert_eq!(
            availability(Action::EditDataset, &target, true),
            Availability::Hidden
        );
        assert_eq!(
            availability(Action::UpdateDataset, &target, true),
            Availability::Hidden
        );
    }

    #[test]
    fn compose_is_offered_for_gray_images_only() {
        let gray = slot_with(test_support::gray_image());
        let rgb = slot_with(test_support::raw_preview());
        let on = |slot| Target::Slot {
            index: 0,
            slot,
            faces: &[],
        };
        assert_eq!(
            availability(Action::ComposeColors, &on(&gray), true),
            Availability::Yes
        );
        assert_eq!(
            availability(Action::ComposeColors, &on(&rgb), true),
            Availability::Hidden
        );
    }

    #[test]
    fn develop_is_offered_for_a_raw_still_showing_its_preview() {
        let raw = slot_with(test_support::raw_preview());
        let gray = slot_with(test_support::gray_image());
        let on = |slot| Target::Slot {
            index: 0,
            slot,
            faces: &[],
        };
        assert_eq!(
            availability(Action::DevelopRaw, &on(&raw), true),
            Availability::Yes
        );
        assert_eq!(
            availability(Action::DevelopRaw, &on(&gray), true),
            Availability::Hidden
        );
    }

    #[test]
    fn measurement_items_appear_only_with_a_selected_point_or_roi() {
        let slot = slot_with(test_support::gray_image());
        let target = Target::Slot {
            index: 0,
            slot: &slot,
            faces: &[],
        };
        assert_eq!(
            availability(Action::CopyMeasurement, &target, true),
            Availability::Hidden
        );
        assert_eq!(
            availability(Action::DeleteProbePoint, &target, true),
            Availability::Hidden
        );
    }

    #[test]
    fn the_menu_title_is_the_short_form_of_the_settings_title() {
        let target = folder(FolderKind::Plain);
        assert_eq!(context_title(Action::CopyFolderPath, &target), "Copy");
        assert_eq!(
            context_title(Action::OpenInFileManager, &target),
            "Open in file manager"
        );
        assert_eq!(
            context_title(Action::AiTagFolder, &target),
            Action::AiTagFolder.title()
        );
    }

    #[test]
    fn the_location_of_a_target_is_what_run_on_receives() {
        assert_eq!(
            folder(FolderKind::Zip).location(),
            Where::Folder(PathBuf::from("/photos/2026"), FolderKind::Zip)
        );
        let slot = Slot::new(3);
        let target = Target::Slot {
            index: 2,
            slot: &slot,
            faces: &[],
        };
        assert_eq!(target.location(), Where::Slot(2));
    }
}
