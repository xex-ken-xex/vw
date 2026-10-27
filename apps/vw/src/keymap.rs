//! ホットキー。操作（[`Action`]）とキーの組み合わせ（[`KeyCombo`]）の対応を持つ。
//!
//! 設定ファイルには `next_image = ["Right", "PageDown", "Space"]` のように
//! 操作名とキーの文字列で保存する。

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use eframe::egui::{Event, Key, Modifiers, PointerButton};
use serde::Serialize;

use crate::layout::SlotLayout;

/// ホットキーを割り当てられる操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    PreviousImage,
    NextImage,
    NewSlot,
    CloseSlot,
    ToggleFolderTree,
    ToggleMetadata,
    ToggleImages,
    ToggleLog,
    ToggleSettings,
    ToggleDatasets,
    CloseAllSlots,
    Quit,
    PreviousSlot,
    NextSlot,
    PreviousPage,
    NextPage,
    ToggleMaximize,
    FitImage,
    ActualSize,
    ZoomIn,
    ZoomOut,
    LayoutAuto,
    Layout1x1,
    Layout1x2,
    Layout2x1,
    Layout2x2,
    Layout3x3,
    Layout4x4,
    LayoutCustom,
    TogglePlay,
    ToggleTagging,
    FlagGood,
    FlagBad,
    Tag1,
    Tag2,
    Tag3,
    Tag4,
    Tag5,
    Tag6,
    Tag7,
    Tag8,
    Tag9,
    FilterAll,
    FilterGood,
    FilterBad,
    FilterUnflagged,
    ExportImages,
    ParentFolder,
    DevelopRaw,
    ReadQrCodes,
    ToggleCalibration,
    NextColorMap,
    ToggleColorGauge,
    TrashBadImages,
    CancelSending,
    SendToApp1,
    SendToApp2,
    SendToApp3,
    SendToApp4,
    SendToApp5,
    SendToApp6,
    SendToApp7,
    SendToApp8,
    SendToApp9,
    FullscreenSlideshow,
    FullscreenExit,
    FullscreenNext,
    FullscreenPrevious,
    FullscreenNextFolder,
    FullscreenPreviousFolder,
    FullscreenPause,
    TogglePixelProbe,
    ToggleRoi,
    ToggleHandTool,
    DeleteProbePoint,
    TogglePixelPipeline,
    ToggleHistogram,
    ToggleZoomMap,
    ToggleMap,
    ToggleSkyChart,
    UndoPosition,
    ShowAbout,
    OpenQualityHelp,
    ShowAiModels,
    AiTagFolder,
    FindFacesFolder,
    RemoveAiTagsFolder,
    TidyAiTags,
    ShowPeople,
    ShowPeopleGraph,
    OpenSettingsFolder,
    OpenModelsFolder,
    ToggleFaceFrames,
}

/// 操作が働く画面。キーの重なりは同じ画面の中でだけ問う。全画面スライドショーの間は
/// ほかの操作が働かないので、← → などを普段の操作と同じキーに割り当てられるようにするため。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Main,
    Fullscreen,
}

/// 設定画面でキーの割り当てを分けて並べる、機能のまとまり。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyGroup {
    Navigate,
    View,
    Layout,
    Image,
    Tools,
    Tagging,
    Filter,
    Send,
    Ai,
    Windows,
    App,
    Fullscreen,
}

impl KeyGroup {
    /// 設定画面に並べる順。
    pub const ALL: [KeyGroup; 12] = [
        KeyGroup::Navigate,
        KeyGroup::View,
        KeyGroup::Layout,
        KeyGroup::Image,
        KeyGroup::Tools,
        KeyGroup::Tagging,
        KeyGroup::Filter,
        KeyGroup::Send,
        KeyGroup::Ai,
        KeyGroup::Windows,
        KeyGroup::App,
        KeyGroup::Fullscreen,
    ];

    pub fn title(self) -> &'static str {
        match self {
            KeyGroup::Navigate => "Move between images and folders",
            KeyGroup::View => "Zoom and play",
            KeyGroup::Layout => "Layout and slots",
            KeyGroup::Image => "Image operations",
            KeyGroup::Tools => "Tools",
            KeyGroup::Tagging => "Tagging",
            KeyGroup::Filter => "Filter, export and trash",
            KeyGroup::Send => "Send to apps",
            KeyGroup::Ai => "AI tags and faces",
            KeyGroup::Windows => "Show and hide windows",
            KeyGroup::App => "Help and settings",
            KeyGroup::Fullscreen => "Fullscreen slideshow",
        }
    }
}

impl Action {
    /// 設定画面に並べる順。
    pub const ALL: [Action; 93] = [
        Action::PreviousImage,
        Action::NextImage,
        Action::PreviousSlot,
        Action::NextSlot,
        Action::PreviousPage,
        Action::NextPage,
        Action::ToggleMaximize,
        Action::ParentFolder,
        Action::TogglePlay,
        Action::FullscreenSlideshow,
        Action::FitImage,
        Action::ActualSize,
        Action::ZoomIn,
        Action::ZoomOut,
        Action::DevelopRaw,
        Action::ReadQrCodes,
        Action::ToggleCalibration,
        Action::NextColorMap,
        Action::ToggleColorGauge,
        Action::LayoutAuto,
        Action::Layout1x1,
        Action::Layout1x2,
        Action::Layout2x1,
        Action::Layout2x2,
        Action::Layout3x3,
        Action::Layout4x4,
        Action::LayoutCustom,
        Action::TogglePixelProbe,
        Action::ToggleRoi,
        Action::ToggleHandTool,
        Action::DeleteProbePoint,
        Action::TogglePixelPipeline,
        Action::ToggleHistogram,
        Action::ToggleZoomMap,
        Action::ToggleMap,
        Action::ToggleSkyChart,
        Action::UndoPosition,
        Action::ToggleTagging,
        Action::FlagGood,
        Action::FlagBad,
        Action::Tag1,
        Action::Tag2,
        Action::Tag3,
        Action::Tag4,
        Action::Tag5,
        Action::Tag6,
        Action::Tag7,
        Action::Tag8,
        Action::Tag9,
        Action::FilterAll,
        Action::FilterGood,
        Action::FilterBad,
        Action::FilterUnflagged,
        Action::ExportImages,
        Action::TrashBadImages,
        Action::CancelSending,
        Action::SendToApp1,
        Action::SendToApp2,
        Action::SendToApp3,
        Action::SendToApp4,
        Action::SendToApp5,
        Action::SendToApp6,
        Action::SendToApp7,
        Action::SendToApp8,
        Action::SendToApp9,
        Action::NewSlot,
        Action::CloseSlot,
        Action::CloseAllSlots,
        Action::Quit,
        Action::ToggleFolderTree,
        Action::ToggleMetadata,
        Action::ToggleImages,
        Action::ToggleLog,
        Action::ToggleSettings,
        Action::ToggleDatasets,
        Action::ShowAbout,
        Action::OpenQualityHelp,
        Action::ShowAiModels,
        Action::AiTagFolder,
        Action::FindFacesFolder,
        Action::RemoveAiTagsFolder,
        Action::TidyAiTags,
        Action::ShowPeople,
        Action::ShowPeopleGraph,
        Action::OpenSettingsFolder,
        Action::OpenModelsFolder,
        Action::ToggleFaceFrames,
        Action::FullscreenNext,
        Action::FullscreenPrevious,
        Action::FullscreenNextFolder,
        Action::FullscreenPreviousFolder,
        Action::FullscreenPause,
        Action::FullscreenExit,
    ];

    /// 設定ファイルでの名前。
    pub fn id(self) -> &'static str {
        match self {
            Action::PreviousImage => "previous_image",
            Action::NextImage => "next_image",
            Action::NewSlot => "new_slot",
            Action::CloseSlot => "close_slot",
            Action::ToggleFolderTree => "toggle_folder_tree",
            Action::ToggleMetadata => "toggle_metadata",
            Action::ToggleImages => "toggle_images",
            Action::ToggleLog => "toggle_log",
            Action::ToggleSettings => "toggle_settings",
            Action::ToggleDatasets => "toggle_datasets",
            Action::CloseAllSlots => "close_all_slots",
            Action::Quit => "quit",
            Action::PreviousSlot => "previous_slot",
            Action::NextSlot => "next_slot",
            Action::PreviousPage => "previous_page",
            Action::NextPage => "next_page",
            Action::ToggleMaximize => "toggle_maximize",
            Action::FitImage => "fit_image",
            Action::ActualSize => "actual_size",
            Action::ZoomIn => "zoom_in",
            Action::ZoomOut => "zoom_out",
            Action::LayoutAuto => "layout_auto",
            Action::Layout1x1 => "layout_1x1",
            Action::Layout1x2 => "layout_1x2",
            Action::Layout2x1 => "layout_2x1",
            Action::Layout2x2 => "layout_2x2",
            Action::Layout3x3 => "layout_3x3",
            Action::Layout4x4 => "layout_4x4",
            Action::LayoutCustom => "layout_custom",
            Action::TogglePlay => "toggle_play",
            Action::ToggleTagging => "toggle_tagging",
            Action::FlagGood => "flag_good",
            Action::FlagBad => "flag_bad",
            Action::Tag1 => "tag_1",
            Action::Tag2 => "tag_2",
            Action::Tag3 => "tag_3",
            Action::Tag4 => "tag_4",
            Action::Tag5 => "tag_5",
            Action::Tag6 => "tag_6",
            Action::Tag7 => "tag_7",
            Action::Tag8 => "tag_8",
            Action::Tag9 => "tag_9",
            Action::FilterAll => "filter_all",
            Action::FilterGood => "filter_good",
            Action::FilterBad => "filter_bad",
            Action::FilterUnflagged => "filter_unflagged",
            Action::ExportImages => "export_images",
            Action::ParentFolder => "parent_folder",
            Action::DevelopRaw => "develop_raw",
            Action::ReadQrCodes => "read_qr_codes",
            Action::ToggleCalibration => "toggle_calibration",
            Action::NextColorMap => "next_color_map",
            Action::ToggleColorGauge => "toggle_color_gauge",
            Action::TrashBadImages => "trash_bad_images",
            Action::CancelSending => "cancel_sending",
            Action::SendToApp1 => "send_to_app_1",
            Action::SendToApp2 => "send_to_app_2",
            Action::SendToApp3 => "send_to_app_3",
            Action::SendToApp4 => "send_to_app_4",
            Action::SendToApp5 => "send_to_app_5",
            Action::SendToApp6 => "send_to_app_6",
            Action::SendToApp7 => "send_to_app_7",
            Action::SendToApp8 => "send_to_app_8",
            Action::SendToApp9 => "send_to_app_9",
            Action::TogglePixelPipeline => "toggle_pixel_pipeline",
            Action::ToggleHistogram => "toggle_histogram",
            Action::ToggleZoomMap => "toggle_zoom_map",
            Action::ToggleMap => "toggle_map",
            Action::ToggleSkyChart => "toggle_sky_chart",
            Action::UndoPosition => "undo_position",
            Action::ShowAbout => "show_about",
            Action::OpenQualityHelp => "open_quality_help",
            Action::ShowAiModels => "show_ai_models",
            Action::AiTagFolder => "ai_tag_folder",
            Action::FindFacesFolder => "find_faces_folder",
            Action::RemoveAiTagsFolder => "remove_ai_tags_folder",
            Action::TidyAiTags => "tidy_ai_tags",
            Action::OpenSettingsFolder => "open_settings_folder",
            Action::OpenModelsFolder => "open_models_folder",
            Action::ShowPeople => "show_people",
            Action::ShowPeopleGraph => "show_people_graph",
            Action::ToggleFaceFrames => "toggle_face_frames",
            Action::FullscreenSlideshow => "fullscreen_slideshow",
            Action::FullscreenExit => "fullscreen_exit",
            Action::FullscreenNext => "fullscreen_next",
            Action::FullscreenPrevious => "fullscreen_previous",
            Action::FullscreenNextFolder => "fullscreen_next_folder",
            Action::FullscreenPreviousFolder => "fullscreen_previous_folder",
            Action::FullscreenPause => "fullscreen_pause",
            Action::TogglePixelProbe => "toggle_pixel_probe",
            Action::ToggleRoi => "toggle_roi",
            Action::ToggleHandTool => "toggle_hand_tool",
            Action::DeleteProbePoint => "delete_probe_point",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }

    /// 画面に出す名前（英語。翻訳は [`crate::i18n`]）。
    pub fn title(self) -> &'static str {
        match self {
            Action::PreviousImage => "Previous image slot",
            Action::NextImage => "Next image slot",
            Action::NewSlot => "New slot",
            Action::CloseSlot => "Close slot",
            Action::ToggleFolderTree => "Show/hide folder tree",
            Action::ToggleMetadata => "Show/hide metadata",
            Action::ToggleImages => "Show/hide images",
            Action::ToggleLog => "Show/hide log",
            Action::ToggleSettings => "Open/close settings",
            Action::ToggleDatasets => "Open/close datasets window",
            Action::CloseAllSlots => "Close all slots",
            Action::Quit => "Quit",
            Action::PreviousSlot => "Previous slot",
            Action::NextSlot => "Next slot",
            Action::PreviousPage => "Previous page",
            Action::NextPage => "Next page",
            Action::ToggleMaximize => "Maximize/restore slot, or open folder",
            Action::FitImage => "Fit image to slot",
            Action::ActualSize => "Actual size (100%)",
            Action::ZoomIn => "Zoom in",
            Action::ZoomOut => "Zoom out",
            Action::DevelopRaw => "Develop RAW from sensor data",
            Action::ReadQrCodes => "Read QR codes",
            Action::ToggleCalibration => "Apply dark and flat",
            Action::NextColorMap => "Next color map",
            Action::ToggleColorGauge => "Color scale gauge",
            Action::LayoutAuto => "Layout: auto",
            Action::Layout1x1 => "Layout: 1 x 1",
            Action::Layout1x2 => "Layout: 1 x 2",
            Action::Layout2x1 => "Layout: 2 x 1",
            Action::Layout2x2 => "Layout: 2 x 2",
            Action::Layout3x3 => "Layout: 3 x 3",
            Action::Layout4x4 => "Layout: 4 x 4",
            Action::LayoutCustom => "Layout: custom",
            Action::TogglePlay => "Play/pause slideshow",
            Action::ToggleTagging => "Tagging mode on/off",
            Action::FlagGood => "Flag as good",
            Action::FlagBad => "Flag as bad",
            Action::Tag1 => "Tag 1",
            Action::Tag2 => "Tag 2",
            Action::Tag3 => "Tag 3",
            Action::Tag4 => "Tag 4",
            Action::Tag5 => "Tag 5",
            Action::Tag6 => "Tag 6",
            Action::Tag7 => "Tag 7",
            Action::Tag8 => "Tag 8",
            Action::Tag9 => "Tag 9",
            Action::FilterAll => "Show all images",
            Action::FilterGood => "Show only good images",
            Action::FilterBad => "Show only bad images",
            Action::FilterUnflagged => "Show only unflagged images",
            Action::ExportImages => "Export images...",
            Action::TrashBadImages => "Move bad images to trash...",
            Action::CancelSending => "Cancel sending",
            Action::SendToApp1 => "Send to app 1",
            Action::SendToApp2 => "Send to app 2",
            Action::SendToApp3 => "Send to app 3",
            Action::SendToApp4 => "Send to app 4",
            Action::SendToApp5 => "Send to app 5",
            Action::SendToApp6 => "Send to app 6",
            Action::SendToApp7 => "Send to app 7",
            Action::SendToApp8 => "Send to app 8",
            Action::SendToApp9 => "Send to app 9",
            Action::TogglePixelPipeline => "Show/hide pixel pipeline",
            Action::ToggleHistogram => "Show/hide analysis window (histogram, evaluation)",
            Action::ToggleZoomMap => "Show/hide zoom map",
            Action::ToggleMap => "Show/hide map",
            Action::ToggleSkyChart => "Show/hide sky chart",
            Action::UndoPosition => "Undo the last position change",
            Action::ShowAbout => "About vw",
            Action::OpenQualityHelp => "Image quality metrics help",
            Action::ShowAiModels => "AI models...",
            Action::AiTagFolder => "Add AI tags to this folder",
            Action::FindFacesFolder => "Find faces in this folder",
            Action::RemoveAiTagsFolder => "Remove AI tags from this folder...",
            Action::TidyAiTags => "Tidy AI tags in this folder...",
            Action::OpenSettingsFolder => "Open the settings folder",
            Action::OpenModelsFolder => "Open the AI models folder",
            Action::ShowPeople => "People...",
            Action::ShowPeopleGraph => "Relationship charts...",
            Action::ToggleFaceFrames => "Show face frames",
            Action::ParentFolder => "Up one folder",
            Action::FullscreenSlideshow => "Fullscreen slideshow...",
            Action::FullscreenExit => "Fullscreen: exit",
            Action::FullscreenNext => "Fullscreen: next image",
            Action::FullscreenPrevious => "Fullscreen: previous image",
            Action::FullscreenNextFolder => "Fullscreen: next folder",
            Action::FullscreenPreviousFolder => "Fullscreen: previous folder",
            Action::FullscreenPause => "Fullscreen: pause/resume",
            Action::TogglePixelProbe => "Pixel probe on/off",
            Action::ToggleRoi => "ROI tool on/off",
            Action::ToggleHandTool => "Hand tool on/off",
            Action::DeleteProbePoint => "Delete selected probe point or ROI",
        }
    }

    /// 設定画面で並べるまとまり。
    pub fn group(self) -> KeyGroup {
        match self {
            Action::PreviousImage
            | Action::NextImage
            | Action::PreviousSlot
            | Action::NextSlot
            | Action::PreviousPage
            | Action::NextPage
            | Action::ParentFolder => KeyGroup::Navigate,
            Action::ToggleMaximize
            | Action::FitImage
            | Action::ActualSize
            | Action::ZoomIn
            | Action::ZoomOut
            | Action::TogglePlay
            | Action::FullscreenSlideshow => KeyGroup::View,
            Action::LayoutAuto
            | Action::Layout1x1
            | Action::Layout1x2
            | Action::Layout2x1
            | Action::Layout2x2
            | Action::Layout3x3
            | Action::Layout4x4
            | Action::LayoutCustom
            | Action::NewSlot
            | Action::CloseSlot
            | Action::CloseAllSlots => KeyGroup::Layout,
            Action::DevelopRaw
            | Action::ReadQrCodes
            | Action::ToggleCalibration
            | Action::NextColorMap
            | Action::ToggleColorGauge => KeyGroup::Image,
            Action::TogglePixelProbe
            | Action::ToggleRoi
            | Action::ToggleHandTool
            | Action::DeleteProbePoint
            | Action::UndoPosition => KeyGroup::Tools,
            Action::ToggleTagging
            | Action::FlagGood
            | Action::FlagBad
            | Action::Tag1
            | Action::Tag2
            | Action::Tag3
            | Action::Tag4
            | Action::Tag5
            | Action::Tag6
            | Action::Tag7
            | Action::Tag8
            | Action::Tag9 => KeyGroup::Tagging,
            Action::FilterAll
            | Action::FilterGood
            | Action::FilterBad
            | Action::FilterUnflagged
            | Action::ExportImages
            | Action::TrashBadImages => KeyGroup::Filter,
            Action::SendToApp1
            | Action::SendToApp2
            | Action::SendToApp3
            | Action::SendToApp4
            | Action::SendToApp5
            | Action::SendToApp6
            | Action::SendToApp7
            | Action::SendToApp8
            | Action::SendToApp9
            | Action::CancelSending => KeyGroup::Send,
            Action::ShowAiModels
            | Action::AiTagFolder
            | Action::FindFacesFolder
            | Action::RemoveAiTagsFolder
            | Action::TidyAiTags
            | Action::ShowPeople
            | Action::ShowPeopleGraph
            | Action::ToggleFaceFrames
            | Action::OpenModelsFolder => KeyGroup::Ai,
            Action::ToggleFolderTree
            | Action::ToggleMetadata
            | Action::ToggleImages
            | Action::ToggleLog
            | Action::ToggleDatasets
            | Action::TogglePixelPipeline
            | Action::ToggleHistogram
            | Action::ToggleZoomMap
            | Action::ToggleMap
            | Action::ToggleSkyChart => KeyGroup::Windows,
            Action::ToggleSettings
            | Action::OpenSettingsFolder
            | Action::ShowAbout
            | Action::OpenQualityHelp
            | Action::Quit => KeyGroup::App,
            Action::FullscreenExit
            | Action::FullscreenNext
            | Action::FullscreenPrevious
            | Action::FullscreenNextFolder
            | Action::FullscreenPreviousFolder
            | Action::FullscreenPause => KeyGroup::Fullscreen,
        }
    }

    pub fn scope(self) -> Scope {
        match self {
            Action::FullscreenExit
            | Action::FullscreenNext
            | Action::FullscreenPrevious
            | Action::FullscreenNextFolder
            | Action::FullscreenPreviousFolder
            | Action::FullscreenPause => Scope::Fullscreen,
            _ => Scope::Main,
        }
    }

    /// キーを押し続けたときの自動リピートで繰り返すか。
    /// 表示切替まで繰り返すと、押している間パネルが点滅してしまう。
    fn repeats(self) -> bool {
        matches!(
            self,
            Action::PreviousImage
                | Action::NextImage
                | Action::PreviousSlot
                | Action::NextSlot
                | Action::PreviousPage
                | Action::NextPage
                | Action::ZoomIn
                | Action::ZoomOut
                | Action::FullscreenNext
                | Action::FullscreenPrevious
                | Action::FullscreenNextFolder
                | Action::FullscreenPreviousFolder
        )
    }

    /// タグのキーなら、タグの一覧での位置（0 から）。
    pub fn tag_index(self) -> Option<usize> {
        const TAGS: [Action; 9] = [
            Action::Tag1,
            Action::Tag2,
            Action::Tag3,
            Action::Tag4,
            Action::Tag5,
            Action::Tag6,
            Action::Tag7,
            Action::Tag8,
            Action::Tag9,
        ];
        TAGS.iter().position(|a| *a == self)
    }

    /// 「送り先のアプリ n」のキーなら、送り先の一覧での位置（0 から）。
    pub fn send_app_index(self) -> Option<usize> {
        const APPS: [Action; 9] = [
            Action::SendToApp1,
            Action::SendToApp2,
            Action::SendToApp3,
            Action::SendToApp4,
            Action::SendToApp5,
            Action::SendToApp6,
            Action::SendToApp7,
            Action::SendToApp8,
            Action::SendToApp9,
        ];
        APPS.iter().position(|a| *a == self)
    }

    /// タグ付けモードの間だけ働く操作。普段は ↑ ↓ や数字キーを押しても何も起きないようにして、
    /// 見ているだけのときにうっかりラベルを付けないため。
    pub fn needs_tagging_mode(self) -> bool {
        matches!(self, Action::FlagGood | Action::FlagBad) || self.tag_index().is_some()
    }

    /// メニューのレイアウト項目が選ぶマス目。`LayoutCustom` は設定の値を使うのでここにはない。
    pub fn layout(self) -> Option<SlotLayout> {
        let [auto, l1x1, l1x2, l2x1, l2x2, l3x3, l4x4] = SlotLayout::PRESETS;
        match self {
            Action::LayoutAuto => Some(auto),
            Action::Layout1x1 => Some(l1x1),
            Action::Layout1x2 => Some(l1x2),
            Action::Layout2x1 => Some(l2x1),
            Action::Layout2x2 => Some(l2x2),
            Action::Layout3x3 => Some(l3x3),
            Action::Layout4x4 => Some(l4x4),
            _ => None,
        }
    }

    /// 以前の版の既定のキー。設定ファイルにこれが残っていれば今の既定のキーに読み替える。
    /// 読み替えないと、新しい操作の既定のキーと重なって、毎回起動時に赤いログが出てしまう。
    fn legacy_default_keys(self) -> Option<Vec<KeyCombo>> {
        use Key::*;
        match self {
            // PageUp / PageDown はページ送りに移した。
            Action::PreviousImage => {
                Some(vec![KeyCombo::plain(ArrowLeft), KeyCombo::plain(PageUp)])
            }
            Action::NextImage => Some(vec![
                KeyCombo::plain(ArrowRight),
                KeyCombo::plain(PageDown),
                KeyCombo::plain(Space),
            ]),
            // マウスの戻るボタンを足した。
            Action::ParentFolder => Some(vec![KeyCombo::plain(Backspace)]),
            _ => None,
        }
    }

    fn default_keys(self) -> Vec<KeyCombo> {
        use Key::*;
        match self {
            Action::PreviousImage => vec![KeyCombo::plain(ArrowLeft)],
            Action::NextImage => vec![KeyCombo::plain(ArrowRight), KeyCombo::plain(Space)],
            Action::NewSlot => vec![KeyCombo::ctrl(N)],
            Action::CloseSlot => vec![KeyCombo::ctrl(W)],
            Action::ToggleFolderTree => vec![KeyCombo::ctrl(Num1)],
            Action::ToggleMetadata => vec![KeyCombo::ctrl(Num2)],
            Action::ToggleImages => vec![KeyCombo::ctrl(Num3)],
            Action::ToggleLog => vec![KeyCombo::ctrl(Num4)],
            Action::ToggleSettings => vec![KeyCombo::ctrl(Comma)],
            Action::ToggleDatasets => vec![KeyCombo::ctrl(D)],
            Action::CloseAllSlots => vec![KeyCombo::ctrl(W).with(true, true, false)],
            Action::Quit => vec![KeyCombo::ctrl(Q)],
            Action::PreviousSlot => vec![KeyCombo::ctrl(ArrowLeft)],
            Action::NextSlot => vec![KeyCombo::ctrl(ArrowRight)],
            Action::PreviousPage => vec![KeyCombo::plain(PageUp)],
            Action::NextPage => vec![KeyCombo::plain(PageDown)],
            Action::ToggleMaximize => vec![KeyCombo::plain(Enter)],
            Action::ParentFolder => vec![
                KeyCombo::plain(Backspace),
                KeyCombo::mouse(MouseButton::Back),
            ],
            Action::FitImage => vec![KeyCombo::plain(Num0)],
            Action::ActualSize => vec![KeyCombo::ctrl(Num0)],
            Action::ZoomIn => vec![KeyCombo::plain(Plus), KeyCombo::plain(Equals)],
            Action::ZoomOut => vec![KeyCombo::plain(Minus)],
            Action::DevelopRaw => vec![KeyCombo::plain(D)],
            Action::ReadQrCodes => vec![],
            Action::ToggleCalibration => vec![],
            Action::NextColorMap => vec![KeyCombo::plain(C)],
            Action::ToggleColorGauge => vec![KeyCombo::plain(G)],
            Action::LayoutAuto => vec![KeyCombo::alt(Num0)],
            Action::Layout1x1 => vec![KeyCombo::alt(Num1)],
            Action::Layout1x2 => vec![KeyCombo::alt(Num2)],
            Action::Layout2x1 => vec![KeyCombo::alt(Num3)],
            Action::Layout2x2 => vec![KeyCombo::alt(Num4)],
            Action::Layout3x3 => vec![KeyCombo::alt(Num5)],
            Action::Layout4x4 => vec![KeyCombo::alt(Num6)],
            Action::LayoutCustom => vec![KeyCombo::alt(Num7)],
            Action::TogglePlay => vec![KeyCombo::plain(P)],
            Action::ToggleTagging => vec![KeyCombo::plain(T)],
            Action::FlagGood => vec![KeyCombo::plain(ArrowUp)],
            Action::FlagBad => vec![KeyCombo::plain(ArrowDown)],
            Action::Tag1 => vec![KeyCombo::plain(Num1)],
            Action::Tag2 => vec![KeyCombo::plain(Num2)],
            Action::Tag3 => vec![KeyCombo::plain(Num3)],
            Action::Tag4 => vec![KeyCombo::plain(Num4)],
            Action::Tag5 => vec![KeyCombo::plain(Num5)],
            Action::Tag6 => vec![KeyCombo::plain(Num6)],
            Action::Tag7 => vec![KeyCombo::plain(Num7)],
            Action::Tag8 => vec![KeyCombo::plain(Num8)],
            Action::Tag9 => vec![KeyCombo::plain(Num9)],
            Action::FilterAll => vec![KeyCombo::ctrl(Num0).with(true, true, false)],
            Action::FilterGood => vec![KeyCombo::ctrl(ArrowUp).with(true, true, false)],
            Action::FilterBad => vec![KeyCombo::ctrl(ArrowDown).with(true, true, false)],
            Action::FilterUnflagged => vec![KeyCombo::ctrl(U).with(true, true, false)],
            Action::ExportImages => vec![KeyCombo::ctrl(E)],
            // 押し間違えで大量に消さないよう、既定ではキーを割り当てない。
            Action::TrashBadImages => vec![],
            Action::CancelSending => vec![],
            Action::SendToApp1 => vec![],
            Action::SendToApp2 => vec![],
            Action::SendToApp3 => vec![],
            Action::SendToApp4 => vec![],
            Action::SendToApp5 => vec![],
            Action::SendToApp6 => vec![],
            Action::SendToApp7 => vec![],
            Action::SendToApp8 => vec![],
            Action::SendToApp9 => vec![],
            Action::TogglePixelPipeline => vec![],
            Action::ToggleHistogram => vec![],
            Action::ToggleZoomMap => vec![],
            Action::ToggleMap => vec![KeyCombo::ctrl(M)],
            Action::ToggleSkyChart => vec![],
            Action::UndoPosition => vec![KeyCombo::ctrl(Z)],
            Action::ShowAbout => vec![],
            Action::OpenQualityHelp => vec![],
            Action::ShowAiModels => vec![],
            Action::AiTagFolder => vec![],
            Action::FindFacesFolder => vec![],
            Action::RemoveAiTagsFolder => vec![],
            Action::TidyAiTags => vec![],
            Action::OpenSettingsFolder => vec![],
            Action::OpenModelsFolder => vec![],
            Action::ShowPeople => vec![],
            Action::ShowPeopleGraph => vec![],
            Action::ToggleFaceFrames => vec![],
            Action::FullscreenSlideshow => vec![KeyCombo::plain(F5)],
            Action::TogglePixelProbe => vec![KeyCombo::plain(V)],
            Action::ToggleRoi => vec![KeyCombo::plain(R)],
            Action::ToggleHandTool => vec![KeyCombo::plain(H)],
            Action::DeleteProbePoint => vec![KeyCombo::plain(Delete)],
            Action::FullscreenExit => vec![KeyCombo::plain(Escape)],
            Action::FullscreenNext => vec![KeyCombo::plain(ArrowRight)],
            Action::FullscreenPrevious => vec![KeyCombo::plain(ArrowLeft)],
            Action::FullscreenNextFolder => vec![KeyCombo::plain(ArrowDown)],
            Action::FullscreenPreviousFolder => vec![KeyCombo::plain(ArrowUp)],
            Action::FullscreenPause => vec![KeyCombo::plain(Space)],
        }
    }
}

/// マウスの横にある戻る・進むのボタン。
///
/// 左・右・中のボタンは割り当てない。画像のパン・拡大縮小や選択にもう使っていて、
/// 割り当てるとそちらの操作と同時に動いてしまうため。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Back,
    Forward,
}

impl MouseButton {
    fn name(self) -> &'static str {
        match self {
            MouseButton::Back => "MouseBack",
            MouseButton::Forward => "MouseForward",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        [MouseButton::Back, MouseButton::Forward]
            .into_iter()
            .find(|b| b.name().eq_ignore_ascii_case(name))
    }

    /// egui の追加ボタン 1・2 は、winit がマウスの戻る・進むとして届けるもの。
    pub fn from_pointer(button: PointerButton) -> Option<Self> {
        match button {
            PointerButton::Extra1 => Some(MouseButton::Back),
            PointerButton::Extra2 => Some(MouseButton::Forward),
            _ => None,
        }
    }
}

/// 割り当てられる入力。キーボードのキーか、マウスの戻る・進むのボタン。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Input {
    Key(Key),
    Mouse(MouseButton),
}

/// 修飾キーとキー（またはマウスのボタン）1 つの組み合わせ。
///
/// Windows キー（Super）は持たない。egui がこの修飾キーを伝えてこないうえ、
/// Windows キーの組み合わせはほぼすべて OS が先に使ってしまうため。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: Input,
}

impl KeyCombo {
    pub const fn plain(key: Key) -> Self {
        Self {
            ctrl: false,
            shift: false,
            alt: false,
            key: Input::Key(key),
        }
    }

    pub const fn mouse(button: MouseButton) -> Self {
        Self {
            ctrl: false,
            shift: false,
            alt: false,
            key: Input::Mouse(button),
        }
    }

    /// マウスのボタンを押したときの組み合わせ。割り当てられないボタンなら `None`。
    pub fn from_pointer(button: PointerButton, modifiers: Modifiers) -> Option<Self> {
        let button = MouseButton::from_pointer(button)?;
        Some(Self {
            ctrl: modifiers.ctrl,
            shift: modifiers.shift,
            alt: modifiers.alt,
            key: Input::Mouse(button),
        })
    }

    pub const fn ctrl(key: Key) -> Self {
        Self {
            ctrl: true,
            ..Self::plain(key)
        }
    }

    pub const fn alt(key: Key) -> Self {
        Self {
            alt: true,
            ..Self::plain(key)
        }
    }

    pub fn from_event(key: Key, modifiers: Modifiers) -> Self {
        Self {
            ctrl: modifiers.ctrl,
            shift: modifiers.shift,
            alt: modifiers.alt,
            key: Input::Key(key),
        }
    }

    const fn with(self, ctrl: bool, shift: bool, alt: bool) -> Self {
        Self {
            ctrl,
            shift,
            alt,
            key: self.key,
        }
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        match self.key {
            Input::Key(key) => f.write_str(key.name()),
            Input::Mouse(button) => f.write_str(button.name()),
        }
    }
}

impl FromStr for KeyCombo {
    type Err = String;

    /// `Ctrl+Shift+O` の形式を読む。修飾キーの大文字小文字と順番は問わない。
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts: Vec<&str> = s.split('+').map(str::trim).collect();
        // "Ctrl++" のようにキー自体が + のとき、split で末尾が空になる。
        if parts.len() >= 2
            && parts[parts.len() - 1].is_empty()
            && parts[parts.len() - 2].is_empty()
        {
            parts.truncate(parts.len() - 2);
            parts.push("+");
        }
        let (key_name, mods) = parts
            .split_last()
            .ok_or_else(|| format!("empty key: {s:?}"))?;
        let key = MouseButton::from_name(key_name)
            .map(Input::Mouse)
            .or_else(|| {
                Key::from_name(key_name)
                    .filter(|k| !is_modifier(*k))
                    .map(Input::Key)
            })
            .ok_or_else(|| format!("unknown key {key_name:?} in {s:?}"))?;
        let mut combo = KeyCombo {
            ctrl: false,
            shift: false,
            alt: false,
            key,
        };
        for m in mods {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => combo.ctrl = true,
                "shift" => combo.shift = true,
                "alt" => combo.alt = true,
                _ => return Err(format!("unknown modifier {m:?} in {s:?}")),
            }
        }
        Ok(combo)
    }
}

/// 修飾キーそのもの。egui は修飾キー単独の押下もキーイベントとして届けるが、
/// それだけでは割り当ての対象にしない（Ctrl を押した瞬間に登録されてしまうため）。
pub fn is_modifier(key: Key) -> bool {
    use Key::*;
    matches!(
        key,
        ShiftLeft
            | ShiftRight
            | ControlLeft
            | ControlRight
            | AltLeft
            | AltRight
            | SuperLeft
            | SuperRight
    )
}

/// 割り当てを断った理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// そのキーは別の操作に割り当て済み。
    UsedBy(Action),
    /// 同じ操作にもう割り当ててある。
    AlreadyAssigned,
}

/// 操作ごとのキーの一覧。設定ファイルにない操作は既定のキーになる。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(into = "BTreeMap<String, Vec<String>>")]
pub struct Keymap {
    bindings: BTreeMap<Action, Vec<KeyCombo>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            bindings: Action::ALL.iter().map(|a| (*a, a.default_keys())).collect(),
        }
    }
}

/// 設定ファイルの 1 操作分（キー名の配列）を読む。1 つでも読めなければ全体を不正とする。
/// 読めたキーだけを残さないのは、残りの割り当てが利用者の意図と違う形で効いてしまうため。
fn parse_keys(value: &toml::Value) -> Result<Vec<KeyCombo>, String> {
    let items = value
        .as_array()
        .ok_or_else(|| format!("expected a list of keys, found {value}"))?;
    let mut combos = Vec::new();
    for item in items {
        let text = item
            .as_str()
            .ok_or_else(|| format!("expected a key name, found {item}"))?;
        let combo = text.parse::<KeyCombo>()?;
        if !combos.contains(&combo) {
            combos.push(combo);
        }
    }
    Ok(combos)
}

impl Keymap {
    /// 設定ファイルの `[keys]` を読む。書かれていない操作は既定のキー。
    /// 読めない操作と、他の操作とキーが重なった操作は既定のキーに戻し、その旨を返す。
    pub fn from_table(table: &toml::Table) -> (Self, Vec<String>) {
        let mut map = Keymap::default();
        let mut problems = Vec::new();
        for (id, value) in table {
            let Some(action) = Action::from_id(id) else {
                problems.push(format!("keys.{id}: unknown action (ignored)"));
                continue;
            };
            match parse_keys(value) {
                Ok(keys) if action.legacy_default_keys().as_ref() == Some(&keys) => {
                    map.bindings.insert(action, action.default_keys());
                }
                Ok(keys) => {
                    map.bindings.insert(action, keys);
                }
                Err(e) => problems.push(format!("keys.{id}: {e} (using default keys)")),
            }
        }
        // 既定に戻した操作のキーが、別の操作に書かれたキーと新たに重なることがあるので、
        // 重なりがなくなるまで繰り返す。既定のキー同士は重ならないので必ず終わる。
        while let Some((combo, actions)) = map.duplicates().into_iter().next() {
            let custom: Vec<Action> = actions
                .iter()
                .copied()
                .filter(|a| !map.is_default(*a))
                .collect();
            if custom.is_empty() {
                break;
            }
            for a in &custom {
                map.bindings.insert(*a, a.default_keys());
            }
            let ids: Vec<&str> = custom.iter().map(|a| a.id()).collect();
            problems.push(format!(
                "keys: {combo} is assigned to several actions; using default keys for {}",
                ids.join(", ")
            ));
        }
        (map, problems)
    }
}

impl From<Keymap> for BTreeMap<String, Vec<String>> {
    fn from(map: Keymap) -> Self {
        map.bindings
            .into_iter()
            .map(|(a, keys)| {
                (
                    a.id().to_owned(),
                    keys.iter().map(KeyCombo::to_string).collect(),
                )
            })
            .collect()
    }
}

impl Keymap {
    pub fn keys(&self, action: Action) -> &[KeyCombo] {
        self.bindings.get(&action).map_or(&[], Vec::as_slice)
    }

    /// 画面 `scope` で `combo` が割り当てられている操作。ファイルを手で編集して重複していたら、
    /// [`Action::ALL`] の順で先のものを返す。
    pub fn action_for(&self, scope: Scope, combo: KeyCombo) -> Option<Action> {
        Action::ALL
            .into_iter()
            .find(|a| a.scope() == scope && self.keys(*a).contains(&combo))
    }

    /// `combo` を `action` に足す。別の操作に割り当て済みのキーは受け付けない。
    pub fn assign(&mut self, action: Action, combo: KeyCombo) -> Result<(), Refusal> {
        match self.action_for(action.scope(), combo) {
            Some(a) if a == action => Err(Refusal::AlreadyAssigned),
            Some(a) => Err(Refusal::UsedBy(a)),
            None => {
                self.bindings.entry(action).or_default().push(combo);
                Ok(())
            }
        }
    }

    pub fn unassign(&mut self, action: Action, combo: KeyCombo) {
        if let Some(keys) = self.bindings.get_mut(&action) {
            keys.retain(|k| *k != combo);
        }
    }

    pub fn is_default(&self, action: Action) -> bool {
        self.keys(action) == action.default_keys()
    }

    /// `action` を既定のキーに戻す。既定のキーが今は別の操作に割り当てられていれば、
    /// 重複を作らないようにそのキーは外す。
    pub fn reset(&mut self, action: Action) {
        let keys = action
            .default_keys()
            .into_iter()
            .filter(|k| {
                self.action_for(action.scope(), *k)
                    .is_none_or(|a| a == action)
            })
            .collect();
        self.bindings.insert(action, keys);
    }

    /// 同じ画面の 2 つ以上の操作に割り当てられているキー。設定ファイルを手で編集したときに起こる。
    pub fn duplicates(&self) -> Vec<(KeyCombo, Vec<Action>)> {
        let mut seen: Vec<(KeyCombo, Vec<Action>)> = Vec::new();
        for a in Action::ALL {
            for k in self.keys(a) {
                let same = |(c, actions): &&mut (KeyCombo, Vec<Action>)| {
                    c == k && actions[0].scope() == a.scope()
                };
                match seen.iter_mut().find(same) {
                    Some((_, actions)) => actions.push(a),
                    None => seen.push((*k, vec![a])),
                }
            }
        }
        seen.retain(|(_, actions)| actions.len() > 1);
        seen
    }

    /// このフレームの入力イベントから、画面 `scope` で実行する操作を順に取り出す。
    pub fn actions_in(&self, scope: Scope, events: &[Event]) -> Vec<Action> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Key {
                    key,
                    pressed: true,
                    repeat,
                    modifiers,
                    ..
                } => self
                    .action_for(scope, KeyCombo::from_event(*key, *modifiers))
                    .filter(|a| !repeat || a.repeats()),
                Event::PointerButton {
                    button,
                    pressed: true,
                    modifiers,
                    ..
                } => KeyCombo::from_pointer(*button, *modifiers)
                    .and_then(|combo| self.action_for(scope, combo)),
                _ => None,
            })
            .collect()
    }
}

/// OS の種類。予約キーの一覧を切り替えるのに使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
}

impl Os {
    pub const CURRENT: Os = if cfg!(windows) {
        Os::Windows
    } else {
        Os::Linux
    };
}

/// `action` に `combo` を割り当てたとき、OS の使い方とぶつかるならその用途を返す。
/// OS と同じ用途（Ctrl+Z の元に戻すなど）に使うなら、ぶつからない。
pub fn os_conflict(os: Os, action: Action, combo: KeyCombo) -> Option<&'static str> {
    let same_use = match action {
        Action::UndoPosition => Some("Undo"),
        _ => None,
    };
    os_reserved(os, combo).filter(|r| Some(*r) != same_use)
}

/// OS やデスクトップ環境が使っているキーなら、その用途を返す。
///
/// 割り当て自体は止めない。デスクトップ環境の設定で空いていることもあり、
/// 使えるかどうかは利用者にしか判断できないため。
/// Ctrl+C / X / V などのクリップボード操作はここにない。egui がキーではなく
/// コピー・貼り付けのイベントとして届けるので、そもそも割り当てられない。
pub fn os_reserved(os: Os, combo: KeyCombo) -> Option<&'static str> {
    use Key::*;
    let k = KeyCombo::plain;
    // (ctrl, shift, alt) の組み合わせを短く書くため。
    let c = |key| k(key).with(true, false, false);
    let a = |key| k(key).with(false, false, true);
    let ca = |key| k(key).with(true, false, true);
    let cs = |key| k(key).with(true, true, false);
    let common: [(KeyCombo, &str); 7] = [
        (a(F4), "Close window"),
        (a(Tab), "Switch windows"),
        (k(Tab).with(false, true, true), "Switch windows"),
        (a(Space), "Window menu"),
        (ca(Delete), "System menu"),
        (c(A), "Select all"),
        (c(Z), "Undo"),
    ];
    let windows: [(KeyCombo, &str); 5] = [
        (cs(Escape), "Task Manager"),
        (c(Escape), "Start menu"),
        (a(Escape), "Switch windows"),
        (ca(Tab), "Switch windows"),
        (c(Y), "Redo"),
    ];
    let linux: [(KeyCombo, &str); 13] = [
        (a(F1), "Application menu"),
        (a(F2), "Run command"),
        (a(F7), "Move window"),
        (a(F8), "Resize window"),
        (a(F10), "Maximize window"),
        (ca(T), "Open terminal"),
        (ca(L), "Lock screen"),
        (ca(Backspace), "Restart display server"),
        (ca(ArrowLeft), "Switch workspace"),
        (ca(ArrowRight), "Switch workspace"),
        (ca(ArrowUp), "Switch workspace"),
        (ca(ArrowDown), "Switch workspace"),
        (cs(Z), "Redo"),
    ];
    let specific: &[(KeyCombo, &str)] = match os {
        Os::Windows => &windows,
        Os::Linux => &linux,
    };
    let vt_switch = os == Os::Linux
        && combo.ctrl
        && combo.alt
        && !combo.shift
        && matches!(
            combo.key,
            Input::Key(F1 | F2 | F3 | F4 | F5 | F6 | F7 | F8 | F9 | F10 | F11 | F12)
        );
    if vt_switch {
        return Some("Switch virtual console");
    }
    common
        .iter()
        .chain(specific)
        .find(|(r, _)| *r == combo)
        .map(|(_, what)| *what)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(s: &str) -> KeyCombo {
        s.parse().unwrap()
    }

    fn key_event(key: Key, modifiers: Modifiers, repeat: bool) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers,
        }
    }

    #[test]
    fn only_fullscreen_actions_are_grouped_under_fullscreen() {
        for a in Action::ALL {
            assert_eq!(
                a.group() == KeyGroup::Fullscreen,
                a.scope() == Scope::Fullscreen,
                "{a:?}"
            );
        }
    }

    #[test]
    fn every_group_has_at_least_one_action() {
        for g in KeyGroup::ALL {
            assert!(Action::ALL.iter().any(|a| a.group() == g), "{g:?}");
        }
    }

    #[test]
    fn combo_round_trips_through_text() {
        for s in [
            "Right",
            "Ctrl+N",
            "Ctrl+Shift+Alt+F5",
            "Ctrl+Comma",
            "Shift+Space",
            "Ctrl+1",
        ] {
            assert_eq!(combo(s).to_string(), s);
        }
    }

    #[test]
    fn combo_text_ignores_modifier_case_and_order() {
        assert_eq!(combo("shift+CTRL+o"), combo("Ctrl+Shift+O"));
        assert_eq!(combo("Control + W"), KeyCombo::ctrl(Key::W));
    }

    #[test]
    fn plus_key_can_be_written_after_a_modifier() {
        assert_eq!(combo("Ctrl++"), KeyCombo::ctrl(Key::Plus));
    }

    #[test]
    fn invalid_combo_text_is_rejected_with_reason() {
        assert!("Ctrl+NoSuchKey"
            .parse::<KeyCombo>()
            .unwrap_err()
            .contains("NoSuchKey"));
        assert!("Hyper+A".parse::<KeyCombo>().unwrap_err().contains("Hyper"));
        assert!("".parse::<KeyCombo>().is_err());
    }

    #[test]
    fn modifier_alone_is_not_a_key() {
        assert!("AltLeft".parse::<KeyCombo>().is_err());
        assert!("Ctrl+ShiftLeft".parse::<KeyCombo>().is_err());
    }

    #[test]
    fn defaults_keep_the_original_navigation_keys() {
        let map = Keymap::default();
        assert_eq!(
            map.action_for(Scope::Main, combo("Right")),
            Some(Action::NextImage)
        );
        assert_eq!(
            map.action_for(Scope::Main, combo("Space")),
            Some(Action::NextImage)
        );
        assert_eq!(
            map.action_for(Scope::Main, combo("Left")),
            Some(Action::PreviousImage)
        );
        assert_eq!(
            map.action_for(Scope::Main, combo("Ctrl+Comma")),
            Some(Action::ToggleSettings)
        );
    }

    #[test]
    fn fullscreen_keys_reuse_main_keys_without_clashing() {
        let map = Keymap::default();
        assert_eq!(
            map.action_for(Scope::Fullscreen, combo("Right")),
            Some(Action::FullscreenNext)
        );
        assert_eq!(
            map.action_for(Scope::Fullscreen, combo("Up")),
            Some(Action::FullscreenPreviousFolder)
        );
        assert_eq!(map.action_for(Scope::Main, combo("Escape")), None);
        assert_eq!(map.action_for(Scope::Fullscreen, combo("Ctrl+Comma")), None);
    }

    #[test]
    fn key_of_another_screen_can_be_assigned() {
        let mut map = Keymap::default();
        assert_eq!(map.assign(Action::FullscreenPause, combo("P")), Ok(()));
        assert_eq!(
            map.assign(Action::FullscreenNext, combo("Down")),
            Err(Refusal::UsedBy(Action::FullscreenNextFolder))
        );
        assert!(map.duplicates().is_empty());
    }

    #[test]
    fn key_events_become_actions_of_the_given_screen_only() {
        let map = Keymap::default();
        let events = [key_event(Key::ArrowRight, Modifiers::NONE, false)];
        assert_eq!(
            map.actions_in(Scope::Fullscreen, &events),
            [Action::FullscreenNext]
        );
    }

    #[test]
    fn defaults_have_no_duplicates() {
        assert!(Keymap::default().duplicates().is_empty());
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let map = Keymap::default();
        assert_eq!(map.action_for(Scope::Main, combo("Shift+Right")), None);
        assert_eq!(map.action_for(Scope::Main, combo("N")), None);
    }

    #[test]
    fn an_action_can_have_several_keys() {
        let mut map = Keymap::default();
        map.assign(Action::NewSlot, combo("Insert")).unwrap();
        assert_eq!(
            map.keys(Action::NewSlot),
            [combo("Ctrl+N"), combo("Insert")]
        );
    }

    #[test]
    fn key_used_by_another_action_is_refused() {
        let mut map = Keymap::default();
        assert_eq!(
            map.assign(Action::NewSlot, combo("Right")),
            Err(Refusal::UsedBy(Action::NextImage))
        );
        assert_eq!(map.keys(Action::NewSlot), [combo("Ctrl+N")]);
    }

    #[test]
    fn same_key_twice_on_one_action_is_refused() {
        let mut map = Keymap::default();
        assert_eq!(
            map.assign(Action::NewSlot, combo("Ctrl+N")),
            Err(Refusal::AlreadyAssigned)
        );
    }

    #[test]
    fn unassigned_key_becomes_free_for_other_actions() {
        let mut map = Keymap::default();
        map.unassign(Action::NextImage, combo("Space"));
        assert_eq!(map.action_for(Scope::Main, combo("Space")), None);
        map.assign(Action::NewSlot, combo("Space")).unwrap();
        assert_eq!(
            map.action_for(Scope::Main, combo("Space")),
            Some(Action::NewSlot)
        );
    }

    #[test]
    fn reset_restores_defaults_without_stealing_keys() {
        let mut map = Keymap::default();
        map.unassign(Action::NextImage, combo("Space"));
        map.assign(Action::NewSlot, combo("Space")).unwrap();
        map.unassign(Action::NextImage, combo("Right"));
        assert!(!map.is_default(Action::NextImage));
        map.reset(Action::NextImage);
        assert_eq!(map.keys(Action::NextImage), [combo("Right")]);
        assert_eq!(
            map.action_for(Scope::Main, combo("Space")),
            Some(Action::NewSlot)
        );
    }

    fn load(text: &str) -> (Keymap, Vec<String>) {
        Keymap::from_table(&text.parse::<toml::Table>().unwrap())
    }

    #[test]
    fn saved_backspace_only_parent_folder_gains_the_mouse_back_button() {
        let (map, problems) = load("parent_folder = [\"Backspace\"]");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            map.action_for(Scope::Main, combo("MouseBack")),
            Some(Action::ParentFolder)
        );
    }

    #[test]
    fn old_default_image_keys_become_the_new_defaults_without_warning() {
        let (map, problems) = load(
            "next_image = [\"Right\", \"PageDown\", \"Space\"]\n\
             previous_image = [\"Left\", \"PageUp\"]",
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            map.action_for(Scope::Main, combo("PageDown")),
            Some(Action::NextPage)
        );
        assert_eq!(
            map.action_for(Scope::Main, combo("PageUp")),
            Some(Action::PreviousPage)
        );
        assert!(map.is_default(Action::NextImage));
    }

    #[test]
    fn page_keys_page_and_arrows_step_images_by_default() {
        let map = Keymap::default();
        assert_eq!(
            map.action_for(Scope::Main, combo("PageDown")),
            Some(Action::NextPage)
        );
        assert_eq!(
            map.action_for(Scope::Main, combo("Right")),
            Some(Action::NextImage)
        );
        assert_eq!(
            map.action_for(Scope::Main, combo("Ctrl+Right")),
            Some(Action::NextSlot)
        );
        assert_eq!(
            map.action_for(Scope::Main, combo("Enter")),
            Some(Action::ToggleMaximize)
        );
    }

    #[test]
    fn file_entries_override_only_the_listed_actions() {
        let (map, problems) = load("new_slot = [\"Insert\"]");
        assert_eq!(map.keys(Action::NewSlot), [combo("Insert")]);
        assert_eq!(map.keys(Action::CloseSlot), [combo("Ctrl+W")]);
        assert!(problems.is_empty());
    }

    #[test]
    fn empty_list_in_file_leaves_action_unbound() {
        let (map, problems) = load("new_slot = []");
        assert!(map.keys(Action::NewSlot).is_empty());
        assert!(problems.is_empty());
    }

    #[test]
    fn unknown_action_is_reported_and_ignored() {
        let (map, problems) = load("fly = [\"F\"]");
        assert_eq!(map, Keymap::default());
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn action_with_any_bad_key_falls_back_to_its_defaults() {
        let (map, problems) = load("new_slot = [\"Ctrl+Nope\", \"Insert\"]");
        assert_eq!(map.keys(Action::NewSlot), [combo("Ctrl+N")]);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn action_with_wrong_value_type_falls_back_to_its_defaults() {
        for text in [
            "new_slot = \"Insert\"",
            "new_slot = [1]",
            "new_slot = { a = 1 }",
        ] {
            let (map, problems) = load(text);
            assert_eq!(map.keys(Action::NewSlot), [combo("Ctrl+N")], "{text}");
            assert_eq!(problems.len(), 1, "{text}");
        }
    }

    #[test]
    fn hand_written_duplicate_reverts_the_customized_action() {
        let (map, problems) = load("new_slot = [\"Right\"]");
        assert_eq!(map, Keymap::default());
        assert!(map.duplicates().is_empty());
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn duplicate_between_two_customized_actions_reverts_both() {
        let (map, problems) = load("new_slot = [\"F2\"]\nclose_slot = [\"F2\"]");
        assert_eq!(map, Keymap::default());
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn reverting_does_not_leave_a_new_duplicate() {
        // next_image が壊れて既定（Right など）に戻ると、Right を書いた new_slot と重なる。
        let (map, _) = load("next_image = [\"Nope\"]\nnew_slot = [\"Right\"]");
        assert!(map.duplicates().is_empty());
        assert_eq!(map, Keymap::default());
    }

    #[test]
    fn saved_form_reads_back_identically() {
        let mut map = Keymap::default();
        map.assign(Action::CloseSlot, combo("Ctrl+Shift+F4"))
            .unwrap();
        let raw: BTreeMap<String, Vec<String>> = map.clone().into();
        let table = toml::Table::try_from(raw).unwrap();
        assert_eq!(Keymap::from_table(&table), (map, vec![]));
    }

    #[test]
    fn key_events_become_actions() {
        let map = Keymap::default();
        let events = [
            key_event(Key::ArrowRight, Modifiers::NONE, false),
            key_event(Key::N, Modifiers::CTRL, false),
            key_event(Key::Q, Modifiers::NONE, false),
        ];
        assert_eq!(
            map.actions_in(Scope::Main, &events),
            [Action::NextImage, Action::NewSlot]
        );
    }

    #[test]
    fn mouse_back_button_goes_to_the_parent_folder() {
        let map = Keymap::default();
        let events = [Event::PointerButton {
            pos: Default::default(),
            button: PointerButton::Extra1,
            pressed: true,
            modifiers: Modifiers::NONE,
        }];
        assert_eq!(map.actions_in(Scope::Main, &events), [Action::ParentFolder]);
    }

    #[test]
    fn primary_mouse_button_is_never_an_action() {
        let map = Keymap::default();
        let events = [Event::PointerButton {
            pos: Default::default(),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }];
        assert!(map.actions_in(Scope::Main, &events).is_empty());
    }

    #[test]
    fn mouse_buttons_round_trip_through_text() {
        for s in ["MouseBack", "Ctrl+MouseForward"] {
            assert_eq!(s.parse::<KeyCombo>().unwrap().to_string(), s);
        }
    }

    #[test]
    fn held_key_repeats_navigation_but_not_toggles() {
        let map = Keymap::default();
        let events = [
            key_event(Key::ArrowRight, Modifiers::NONE, true),
            key_event(Key::Num4, Modifiers::CTRL, true),
        ];
        assert_eq!(map.actions_in(Scope::Main, &events), [Action::NextImage]);
    }

    #[test]
    fn os_window_keys_are_flagged_on_both_platforms() {
        for os in [Os::Windows, Os::Linux] {
            assert_eq!(os_reserved(os, combo("Alt+F4")), Some("Close window"));
            assert_eq!(os_reserved(os, combo("Alt+Tab")), Some("Switch windows"));
        }
    }

    #[test]
    fn platform_specific_keys_are_flagged_only_on_that_platform() {
        assert_eq!(
            os_reserved(Os::Windows, combo("Ctrl+Shift+Escape")),
            Some("Task Manager")
        );
        assert_eq!(os_reserved(Os::Linux, combo("Ctrl+Shift+Escape")), None);
        assert_eq!(
            os_reserved(Os::Linux, combo("Ctrl+Alt+T")),
            Some("Open terminal")
        );
        assert_eq!(os_reserved(Os::Windows, combo("Ctrl+Alt+T")), None);
        assert_eq!(
            os_reserved(Os::Linux, combo("Ctrl+Alt+F3")),
            Some("Switch virtual console")
        );
    }

    #[test]
    fn ordinary_keys_are_not_flagged() {
        for os in [Os::Windows, Os::Linux] {
            for s in ["Right", "Ctrl+N", "Ctrl+Comma", "F5", "Shift+Alt+F4"] {
                assert_eq!(os_reserved(os, combo(s)), None, "{s}");
            }
        }
    }

    #[test]
    fn default_keys_avoid_os_keys() {
        let map = Keymap::default();
        for os in [Os::Windows, Os::Linux] {
            for a in Action::ALL {
                for k in map.keys(a) {
                    assert_eq!(os_conflict(os, a, *k), None, "{k}");
                }
            }
        }
    }
}
