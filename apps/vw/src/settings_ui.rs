//! 設定画面。VSCode のように、検索欄の語で項目を絞り込める。
//!
//! 値を変えたらその場で反映・保存する（呼び出し側が [`Response::changed`] を見て行う）。
//! 「適用」ボタンを置かないのは、変更の結果を見ながら調整できるようにするため。

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Align, Color32, Event, RichText};
use egui_phosphor::regular as icon;

use crate::i18n::{tr, tr1, tr_text};
use crate::keymap::{
    is_modifier, os_conflict, Action, KeyCombo, KeyGroup, Keymap, Os, Refusal, Scope,
};
use crate::layout::{SlotLayout, WheelScroll};
use vw_quality::Thresholds;

use crate::settings::{
    parse_hex_color, CalibrationApply, ColorFormat, ColorNode, CustomPalette, DarkScaling,
    FitsCalibration, FitsCurve, FitsStretch, FlatMatch, FlatNormalize, Fullscreen, FullscreenMode,
    GaugeSide, HeaderClick, HeadingStyle, Labels, Language, LoadInto, LoadModifier, LogSettings,
    LogStyle, MonitorSettings, PathDisplay, SearchGroup, Settings, ThemeChoice, Tree, Viewer,
};
use crate::slideshow::{EndMode, DROP_WINDOW_RANGE, FPS_RANGE};
use crate::sort::SortKey;
use crate::viewer::ScaleFilter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Appearance,
    Viewer,
    Display,
    Tree,
    Behavior,
    Metadata,
    Labels,
    Datasets,
    Fullscreen,
    Pipeline,
    Fits,
    Dicom,
    SendApps,
    DicomSend,
    Log,
    Quality,
    Ai,
    Map,
    Sky,
    Keys,
}

impl Section {
    const ALL: [Section; 20] = [
        Section::Appearance,
        Section::Viewer,
        Section::Display,
        Section::Tree,
        Section::Behavior,
        Section::Metadata,
        Section::Labels,
        Section::Datasets,
        Section::Fullscreen,
        Section::Pipeline,
        Section::Fits,
        Section::Dicom,
        Section::SendApps,
        Section::DicomSend,
        Section::Log,
        Section::Quality,
        Section::Ai,
        Section::Map,
        Section::Sky,
        Section::Keys,
    ];

    fn title(self) -> &'static str {
        match self {
            Section::Appearance => "Appearance",
            Section::Viewer => "Viewer",
            Section::Display => "Display",
            Section::Tree => "Folder tree",
            Section::Behavior => "Behavior",
            Section::Metadata => "Metadata window",
            Section::Labels => "Flags and tags",
            Section::Datasets => "Datasets",
            Section::Fullscreen => "Fullscreen slideshow",
            Section::Pipeline => "Pixel pipeline",
            Section::Fits => "Astronomy images (FITS)",
            Section::Dicom => "Medical images (DICOM)",
            Section::SendApps => "Send to apps",
            Section::DicomSend => "DICOM sending",
            Section::Log => "Log",
            Section::Quality => "Quality evaluation",
            Section::Ai => "AI tags",
            Section::Map => "Map",
            Section::Sky => "Sky chart",
            Section::Keys => "Keyboard shortcuts",
        }
    }
}

/// 設定項目 1 つ。`widget` は値を編集する部品を描き、変更があれば true を返す。
struct Item {
    section: Section,
    /// 設定ファイルでの場所。
    key: &'static str,
    title: &'static str,
    description: &'static str,
    widget: fn(&mut Settings, &mut egui::Ui, Language) -> bool,
}

const ITEMS: [Item; 106] = [
    Item {
        section: Section::Appearance,
        key: "appearance.theme",
        title: "Theme",
        description: "Follow the OS dark/light setting, or always use one.",
        widget: theme_widget,
    },
    Item {
        section: Section::Appearance,
        key: "appearance.language",
        title: "Language",
        description: "Language of menus and labels.",
        widget: language_widget,
    },
    Item {
        section: Section::Appearance,
        key: "appearance.ui_scale",
        title: "UI scale",
        description: "Size of text and icons.",
        widget: scale_widget,
    },
    Item {
        section: Section::Appearance,
        key: "appearance.compact",
        title: "Compact windows",
        description: "Reduce padding to show more information.",
        widget: compact_widget,
    },
    Item {
        section: Section::Viewer,
        key: "viewer.background",
        title: "Background color",
        description: "Color behind images (#RRGGBB). Leave empty for the theme default.",
        widget: background_widget,
    },
    Item {
        section: Section::Viewer,
        key: "viewer.full_image_limit_mb",
        title: "Full-resolution memory limit",
        description: "Memory used for full-resolution images (MB). Beyond this, images are shown from 512 px thumbnails until memory frees up.",
        widget: limit_widget,
    },
    Item {
        section: Section::Viewer,
        key: "viewer.image_size_limit_mb",
        title: "Image size limit",
        description: "Memory one image may take to decode (MB). A file whose header claims a larger image is not opened, so a broken header cannot exhaust memory.",
        widget: image_size_limit_widget,
    },
    Item {
        section: Section::Viewer,
        key: "viewer.layout",
        title: "Slot layout",
        description: "How slots are arranged. Auto fits every slot on one page up to the Auto layout maximum; a fixed grid shows the rest on further pages (Page Up / Page Down).",
        widget: layout_widget,
    },
    Item {
        section: Section::Viewer,
        key: "viewer.custom_layout",
        title: "Custom layout",
        description: "Columns x rows used by the Custom layout in the View menu.",
        widget: custom_layout_widget,
    },
    Item {
        section: Section::Viewer,
        key: "viewer.auto_layout_max",
        title: "Auto layout maximum",
        description: "Largest grid (columns x rows) the Auto layout uses. More slots than this go on further pages, so slots do not get too small.",
        widget: auto_layout_max_widget,
    },
    Item {
        section: Section::Viewer,
        key: "viewer.page_wheel",
        title: "Wheel scroll amount",
        description: "How far the wheel over the page bar, or Shift+wheel over the slots, moves the slots when they are split into pages: one slot, half a page or one page. The page buttons and Page Up / Page Down always turn one page.",
        widget: page_wheel_widget,
    },
    Item {
        section: Section::Viewer,
        key: "viewer.slideshow_fps",
        title: "Slideshow speed",
        description: "Frames per second of the slideshow. The play bar of a maximized slot changes and remembers it.",
        widget: fps_widget,
    },
    Item {
        section: Section::Viewer,
        key: "viewer.slideshow_end",
        title: "Slideshow end",
        description: "What happens after the last image: start over, stop, or play backwards.",
        widget: end_mode_widget,
    },
    Item {
        section: Section::Viewer,
        key: "viewer.slideshow_drop_window_secs",
        title: "Dropped frames period",
        description: "The play bar shows how many frames were skipped (not decoded in time) during this many last seconds.",
        widget: drop_window_widget,
    },
    Item {
        section: Section::Viewer,
        key: "viewer.keep_view",
        title: "Keep zoom when stepping",
        description: "Keep the zoom and position when moving to the previous or next image in a slot that holds several images (a folder, frames or a series). DICOM images always keep them.",
        widget: keep_view_widget,
    },
    Item {
        section: Section::Viewer,
        key: "viewer.develop_raw",
        title: "Develop RAW in the background",
        description: "Camera RAW files first show the preview JPEG embedded by the camera, then are developed from the sensor data in the background and replaced (a film icon is shown on the slot meanwhile). When off, develop from the right-click menu.",
        widget: develop_raw_widget,
    },
    Item {
        section: Section::Viewer,
        key: "viewer.magnify_filter",
        title: "Zoom-in interpolation",
        description: "How an image enlarged on screen is drawn. Nearest neighbor shows each pixel as a sharp square with its original value; linear blends neighboring pixels smoothly.",
        widget: magnify_filter_widget,
    },
    Item {
        section: Section::Viewer,
        key: "viewer.minify_filter",
        title: "Zoom-out interpolation",
        description: "How an image shrunk on screen is drawn. Linear averages pixels smoothly; nearest neighbor picks single pixels, keeping original values but showing jagged edges and moire.",
        widget: minify_filter_widget,
    },
    Item {
        section: Section::Display,
        key: "display.monitors",
        title: "Monitors",
        description: "For each monitor, draw 16-bit images (PNG, TIFF, FITS, developed RAW...) with 10-bit gradation, or dither them when reducing them to 8 bits so smooth gradients show no bands. Applies to the one large image (1x1, maximized) and the fullscreen slideshow. 10-bit takes effect from the next start.",
        widget: monitors_widget,
    },
    Item {
        section: Section::Tree,
        key: "tree.sort",
        title: "Sort order",
        description: "Order of the folders in the tree and of the folders and images in the slots, also used for previous/next image. Folders always come first.",
        widget: sort_widget,
    },
    Item {
        section: Section::Tree,
        key: "tree.sort_descending",
        title: "Descending",
        description: "Reverse the sort order (Z to A, newest first).",
        widget: descending_widget,
    },
    Item {
        section: Section::Tree,
        key: "tree.confirm_every",
        title: "Confirm large loads",
        description: "Ask before continuing each time the number of loaded images reaches the chunk size.",
        widget: confirm_widget,
    },
    Item {
        section: Section::Tree,
        key: "tree.confirm_chunk",
        title: "Chunk size",
        description: "Number of images to load before asking again.",
        widget: chunk_widget,
    },
    Item {
        section: Section::Tree,
        key: "tree.load_into",
        title: "Load folders into",
        description: "Put each image of the current folder into its own slot, or all of them into one slot that steps through them.",
        widget: load_into_widget,
    },
    Item {
        section: Section::Tree,
        key: "tree.other_load_modifier",
        title: "Key for the other way",
        description: "Hold this key while loading a folder to load it the other way.",
        widget: other_load_modifier_widget,
    },
    Item {
        section: Section::Tree,
        key: "tree.click_expands",
        title: "Expand on click",
        description: "Clicking a folder name also expands it, like Explorer. When off, only the arrow expands or collapses.",
        widget: click_expands_widget,
    },
    Item {
        section: Section::Behavior,
        key: "behavior.header_click",
        title: "Group heading click",
        description: "What clicking a group heading (such as File in the metadata window) does. The arrow always expands or collapses.",
        widget: header_click_widget,
    },
    Item {
        section: Section::Behavior,
        key: "behavior.auto_reload",
        title: "Auto reload",
        description: "Watch open folders. New images appear in the tree and folder slots, a slot showing the last image moves to the newest one, and overwritten images are reloaded.",
        widget: auto_reload_widget,
    },
    Item {
        section: Section::Behavior,
        key: "behavior.archive_dir",
        title: "Archive extraction folder",
        description: "Folder where ZIP and 7z files are extracted while they are open (a vw-archives folder is made inside it). Leave empty for the OS temporary folder. Archives already open stay where they are until closed.",
        widget: archive_dir_widget,
    },
    Item {
        section: Section::Behavior,
        key: "behavior.archive_size_limit_gb",
        title: "Archive size limit",
        description: "Archives larger than this when extracted (GB) are extracted only after you confirm. Archives that do not fit in the free space of the extraction folder are not extracted.",
        widget: archive_size_limit_widget,
    },
    Item {
        section: Section::Metadata,
        key: "metadata.path",
        title: "Path",
        description: "What the Path row of the metadata window shows: the full path (its head is left out when it does not fit) or only the file name. Hover the row to see the full path.",
        widget: path_display_widget,
    },
    Item {
        section: Section::Metadata,
        key: "metadata.hidden",
        title: "Hidden items",
        description: "Items and groups hidden from the metadata window. Right-click an item or a group heading in the window to hide it; press the x here to show it again. The eye button at the top of the window shows hidden items faint for a while.",
        widget: hidden_widget,
    },
    Item {
        section: Section::Metadata,
        key: "metadata.groups",
        title: "Your groups",
        description: "Groups you made by right-clicking items and choosing Add to group. They are shown at the top of the window, one per line as name = group/item; group/item. Edit a line to rename a group or reorder its items, delete it to remove the group.",
        widget: metadata_groups_widget,
    },
    Item {
        section: Section::Metadata,
        key: "metadata.heading_style",
        title: "Group headings",
        description: "How group headings such as File stand out from the items under them.",
        widget: heading_style_widget,
    },
    Item {
        section: Section::Metadata,
        key: "metadata.remember_open",
        title: "Remember open groups",
        description: "Keep groups you closed closed at the next start. When off, every group starts open.",
        widget: remember_open_widget,
    },
    Item {
        section: Section::Metadata,
        key: "metadata.color_format",
        title: "Color values",
        description: "How the color of a pixel is written next to a probe cross and when it is copied: R G B numbers or a color code (#RRGGBB; 16-bit images use four digits per color).",
        widget: probe_color_widget,
    },
    Item {
        section: Section::Labels,
        key: "labels.tags",
        title: "Tags",
        description: "Tags you can put on images, separated by commas. The first nine are put on with the keys 1 to 9 in tagging mode.",
        widget: tags_widget,
    },
    Item {
        section: Section::Labels,
        key: "labels.advance",
        title: "Next image after labeling",
        description: "Move to the next image after putting a flag or tag on an image in tagging mode.",
        widget: advance_widget,
    },
    Item {
        section: Section::Datasets,
        key: "dataset.roots",
        title: "Dataset folders",
        description: "Folders searched for datasets (folders with a search catalog), one per line. Subfolders are searched too.",
        widget: dataset_roots_widget,
    },
    Item {
        section: Section::Datasets,
        key: "dataset.fields",
        title: "Dataset fields",
        description: "Fields you fill in for each dataset besides the name, memo and tags (such as where the data came from), separated by commas. They can be used as search conditions.",
        widget: dataset_fields_widget,
    },
    Item {
        section: Section::Datasets,
        key: "dataset.search_groups",
        title: "Search groups",
        description: "Sets of metadata keys offered in the advanced search, one per line as name = key; key; key. Picking one adds an \"equals\" condition for each key, so you only type the values. Keys are tag numbers (0010,0020) or names (PatientID); put kind: in front to limit the kind (exif:Model).",
        widget: search_groups_widget,
    },
    Item {
        section: Section::Log,
        key: "log.style",
        title: "Look",
        description: "Normal shows the time and the message in the usual font. Terminal shows the time since start and the message in a monospace font on black. Also switchable at the top right of the log.",
        widget: log_style_widget,
    },
    Item {
        section: Section::Log,
        key: "log.file",
        title: "Write to a file",
        description: "Also write every log line with its date, time and level to a file in the logs folder next to the settings file, one file per start. Takes effect at the next start.",
        widget: log_file_widget,
    },
    Item {
        section: Section::Log,
        key: "log.keep_files",
        title: "Log files kept",
        description: "How many log files to keep. Older ones are deleted at start.",
        widget: log_keep_widget,
    },
    Item {
        section: Section::Log,
        key: "log.wrap",
        title: "Wrap long lines",
        description: "Wrap long log lines at the width of the log. When off, scroll sideways to read them. Also switchable at the top right of the log.",
        widget: log_wrap_widget,
    },
    Item {
        section: Section::Quality,
        key: "roi.color_format",
        title: "ROI color values",
        description: "How the mean color of an ROI on a color image is written: R G B numbers or a color code (#RRGGBB; 16-bit images use four digits per color).",
        widget: roi_color_widget,
    },
    Item {
        section: Section::Quality,
        key: "quality.psnr",
        title: "PSNR verdict",
        description: "Peak signal-to-noise ratio against the reference image, in dB. Higher is better.",
        widget: quality_psnr_widget,
    },
    Item {
        section: Section::Quality,
        key: "quality.ssim",
        title: "SSIM verdict",
        description: "Structural similarity against the reference image, 0 to 1. Higher is better.",
        widget: quality_ssim_widget,
    },
    Item {
        section: Section::Quality,
        key: "quality.snr",
        title: "SNR verdict",
        description: "Signal-to-noise ratio of the target ROI (mean / standard deviation). Higher is better.",
        widget: quality_snr_widget,
    },
    Item {
        section: Section::Quality,
        key: "quality.cnr",
        title: "CNR verdict",
        description: "Contrast-to-noise ratio between the target and background ROIs. Higher is better.",
        widget: quality_cnr_widget,
    },
    Item {
        section: Section::Quality,
        key: "quality.sharpness",
        title: "Sharpness verdict",
        description: "Variance of the Laplacian over the whole image. Blur lowers it.",
        widget: quality_sharpness_widget,
    },
    Item {
        section: Section::Ai,
        key: "ai.general_threshold",
        title: "General tag threshold",
        description: "Confidence (0 to 1) a general tag needs to be added. Lower adds more tags.",
        widget: ai_general_widget,
    },
    Item {
        section: Section::Ai,
        key: "ai.character_threshold",
        title: "Character tag threshold",
        description: "Confidence (0 to 1) a character tag needs to be added.",
        widget: ai_character_widget,
    },
    Item {
        section: Section::Ai,
        key: "ai.face_threshold",
        title: "Face match threshold",
        description: "Similarity (0 to 1) a face needs to be named as a registered person. Higher means fewer but surer matches.",
        widget: ai_face_widget,
    },
    Item {
        section: Section::Ai,
        key: "ai.excluded_tags",
        title: "Excluded tags",
        description: "Tags never added even when the tagger finds them, one per line. Right-click a tag in the metadata window to add it here.",
        widget: ai_excluded_widget,
    },
    Item {
        section: Section::Ai,
        key: "ai.threads",
        title: "Inference threads",
        description: "CPU threads used by the tagger. 0 lets ONNX Runtime decide.",
        widget: ai_threads_widget,
    },
    Item {
        section: Section::Ai,
        key: "ai.face_known_color",
        title: "Frame color of known faces",
        description: "Color of the frame and name of faces matched to a registered person (#RRGGBB).",
        widget: face_known_color_widget,
    },
    Item {
        section: Section::Ai,
        key: "ai.face_unknown_color",
        title: "Frame color of unknown faces",
        description: "Color of the frame of faces not matched to anyone (#RRGGBB).",
        widget: face_unknown_color_widget,
    },
    Item {
        section: Section::Fullscreen,
        key: "fullscreen.mode",
        title: "Mode",
        description: "Show the images folder by folder, or keep showing the newest file under the folder and switch as soon as a new one arrives (for tethered shooting).",
        widget: fullscreen_mode_widget,
    },
    Item {
        section: Section::Fullscreen,
        key: "fullscreen.sort",
        title: "Sort by",
        description: "Order of the folders and of the images in each folder.",
        widget: fullscreen_sort_widget,
    },
    Item {
        section: Section::Fullscreen,
        key: "fullscreen.folders_descending",
        title: "Folders descending",
        description: "Go through the folders in reverse order (Z to A, newest first).",
        widget: folders_descending_widget,
    },
    Item {
        section: Section::Fullscreen,
        key: "fullscreen.files_descending",
        title: "Images descending",
        description: "Show the images in each folder in reverse order (Z to A, newest first).",
        widget: files_descending_widget,
    },
    Item {
        section: Section::Fullscreen,
        key: "fullscreen.interval_secs",
        title: "Interval",
        description: "Seconds before moving on to the next image. 0 moves only with the keys. Animations and multi-frame images play at the file's speed before moving on; with 0 only their first frame is shown.",
        widget: interval_widget,
    },
    Item {
        section: Section::Fullscreen,
        key: "fullscreen.loop_at_end",
        title: "Start over at the end",
        description: "After the last image, go back to the first one instead of stopping.",
        widget: loop_at_end_widget,
    },
    Item {
        section: Section::Fullscreen,
        key: "fullscreen.show_path",
        title: "Show path",
        description: "Show the path from the folder and the position in the folder at the bottom right.",
        widget: show_path_widget,
    },
    Item {
        section: Section::Fullscreen,
        key: "fullscreen.path_color",
        title: "Path color",
        description: "Color of the path text (#RRGGBB).",
        widget: path_color_widget,
    },
    Item {
        section: Section::Fullscreen,
        key: "fullscreen.path_opacity",
        title: "Path opacity",
        description: "Opacity of the path text (%).",
        widget: path_opacity_widget,
    },
    Item {
        section: Section::Fullscreen,
        key: "fullscreen.path_size",
        title: "Path text size",
        description: "Size of the path text.",
        widget: path_size_widget,
    },
    Item {
        section: Section::Pipeline,
        key: "pipeline.color_node",
        title: "Color map stage",
        description: "Insert the color map stage into the pixel pipeline of gray images from the start, for the kinds of images turned on below. The stage turns brightness into colors and also changes the slot; probe, ROI and analysis values stay the file values. Each slot can still turn it on or off and pick another color map (right-click, Image menu, or the stage in the pixel pipeline).",
        widget: pipeline_color_node_widget,
    },
    Item {
        section: Section::Pipeline,
        key: "pipeline.fits",
        title: "FITS images",
        description: "Astronomy images: insert the color map stage from the start, and its color map.",
        widget: pipeline_fits_widget,
    },
    Item {
        section: Section::Pipeline,
        key: "pipeline.dicom",
        title: "DICOM images",
        description: "Gray DICOM images other than the subtypes below (CT, MR, X-ray and so on): insert the color map stage from the start, and its color map.",
        widget: pipeline_dicom_widget,
    },
    Item {
        section: Section::Pipeline,
        key: "pipeline.dicom.pt",
        title: "DICOM PT",
        description: "PET images (Modality PT): insert the color map stage from the start, and its color map. Hot Iron, PET and Hot Metal Blue are the palettes defined by the DICOM standard.",
        widget: pipeline_pt_widget,
    },
    Item {
        section: Section::Pipeline,
        key: "pipeline.gray",
        title: "Other gray images",
        description: "Gray images in other formats (16-bit PNG, TIFF and so on): insert the color map stage from the start, and its color map.",
        widget: pipeline_gray_widget,
    },
    Item {
        section: Section::Pipeline,
        key: "pipeline.gauge_side",
        title: "Color scale gauge",
        description: "Where the gauge (the colors with the values of black, white and between) is shown inside the slot. Show or hide it with the button in the left tool column or the G key. Drag its ends to change the black and white values of that slot.",
        widget: pipeline_gauge_side_widget,
    },
    Item {
        section: Section::Pipeline,
        key: "pipeline.gauge_on_gray",
        title: "Gauge on gray images",
        description: "Also show the gauge on gray images without a color map, to read the values of black and white.",
        widget: pipeline_gauge_gray_widget,
    },
    Item {
        section: Section::Pipeline,
        key: "pipeline.palettes",
        title: "Custom color maps",
        description: "Your own color maps, one per line: a name, =, then colors as position (0 to 1) and #rrggbb separated by ;. Colors between are blended. Example: Ice = 0 #000000; 0.5 #0080ff; 1 #ffffff",
        widget: pipeline_palettes_widget,
    },
    Item {
        section: Section::Fits,
        key: "fits.stretch",
        title: "First black and white",
        description: "How the black and white points are chosen when an image opens. Percentiles uses the percentiles below; zscale (as in ds9 and astropy) looks at the values around the sky background; Minimum to maximum often leaves sky images almost black.",
        widget: fits_stretch_widget,
    },
    Item {
        section: Section::Fits,
        key: "fits.low_percent",
        title: "Black percentile",
        description: "With Percentiles, this percent of the pixels are drawn black (%).",
        widget: fits_low_widget,
    },
    Item {
        section: Section::Fits,
        key: "fits.high_percent",
        title: "White percentile",
        description: "With Percentiles, pixels above this percent are drawn white (%).",
        widget: fits_high_widget,
    },
    Item {
        section: Section::Fits,
        key: "fits.curve",
        title: "Curve between black and white",
        description: "How the values between black and white are brightened. Linear keeps them in proportion; asinh, Logarithm and Square root lift faint parts such as nebulae while keeping bright stars from turning white (strongest first).",
        widget: fits_curve_widget,
    },
    Item {
        section: Section::Fits,
        key: "fits.flip_rows",
        title: "Turn upside down",
        description: "Show the first row of the file at the bottom, as the FITS standard and ds9 do. Only for files without ROWORDER; files that say TOP-DOWN or BOTTOM-UP are always shown that way.",
        widget: fits_flip_widget,
    },
    Item {
        section: Section::Fits,
        key: "fits.demosaic",
        title: "Color from the Bayer pattern",
        description: "Turn images from color cameras (with BAYERPAT) into color. When off, they are shown as the gray mosaic stored in the file.",
        widget: fits_demosaic_widget,
    },
    Item {
        section: Section::Fits,
        key: "fits.calibration.apply",
        title: "Dark and flat",
        description: "When dark, flat and bias masters that fit an opened light frame are found, apply them before the colors and the stretch. The slot shows a mark, and Apply dark and flat (right-click or a key) shows the image without them to compare.",
        widget: calib_apply_widget,
    },
    Item {
        section: Section::Fits,
        key: "fits.calibration.search_nearby",
        title: "Look near the opened folder",
        description: "Look for masters in the opened folder, its parent, and folders named darks, flats, biases, darkflats, calibration or masters next to them (as N.I.N.A. and Siril arrange them).",
        widget: calib_nearby_widget,
    },
    Item {
        section: Section::Fits,
        key: "fits.calibration.library",
        title: "Calibration library",
        description: "Folders where masters are kept and reused across nights, one per line. Subfolders are searched too.",
        widget: calib_library_widget,
    },
    Item {
        section: Section::Fits,
        key: "fits.calibration.use_dark",
        title: "Use darks",
        description: "Subtract a master dark from the light.",
        widget: calib_use_dark_widget,
    },
    Item {
        section: Section::Fits,
        key: "fits.calibration.use_flat",
        title: "Use flats",
        description: "Divide the light by a master flat.",
        widget: calib_use_flat_widget,
    },
    Item {
        section: Section::Fits,
        key: "fits.calibration.use_bias",
        title: "Use biases",
        description: "Subtract a master bias from the flat (when there is no dark flat) and from the light (when there is no dark).",
        widget: calib_use_bias_widget,
    },
    Item {
        section: Section::Fits,
        key: "fits.calibration.exposure_tolerance_secs",
        title: "Exposure difference allowed",
        description: "A dark is used only when its EXPTIME differs from the light's by at most this (seconds).",
        widget: calib_exposure_widget,
    },
    Item {
        section: Section::Fits,
        key: "fits.calibration.temperature_tolerance",
        title: "Temperature difference allowed",
        description: "A dark is used only when its CCD-TEMP (or SET-TEMP) differs from the light's by at most this (°C). Files without a temperature are not compared.",
        widget: calib_temperature_widget,
    },
    Item {
        section: Section::Fits,
        key: "fits.calibration.match_gain_offset",
        title: "Match GAIN and OFFSET",
        description: "Use only darks and biases with the light's GAIN and OFFSET, when both files have them.",
        widget: calib_gain_widget,
    },
    Item {
        section: Section::Fits,
        key: "fits.calibration.dark_scaling",
        title: "Darks of another exposure",
        description: "Same exposure only is the usual way, because the amp glow of CMOS sensors does not grow with the exposure. Scale by exposure stretches a dark of another exposure after removing the bias (needs a bias or a constant bias).",
        widget: calib_scaling_widget,
    },
    Item {
        section: Section::Fits,
        key: "fits.calibration.flat_match",
        title: "Choosing flats",
        description: "Flats must have the light's size, binning and FILTER. With Same night too, they must also be taken the same night (noon to noon).",
        widget: calib_flat_match_widget,
    },
    Item {
        section: Section::Fits,
        key: "fits.calibration.flat_normalize",
        title: "Flat normalized by",
        description: "The flat is divided by this value so that it is about 1.",
        widget: calib_normalize_widget,
    },
    Item {
        section: Section::Fits,
        key: "fits.calibration.equalize_cfa",
        title: "Even out color flats",
        description: "Normalize the flat of a color camera separately for each position of the Bayer pattern, which removes the color cast of the flat panel.",
        widget: calib_equalize_widget,
    },
    Item {
        section: Section::Fits,
        key: "fits.calibration.synthetic_bias",
        title: "Constant bias",
        description: "A value subtracted instead of a master bias when there is none (for example 2048). Empty: nothing.",
        widget: calib_synthetic_widget,
    },
    Item {
        section: Section::Dicom,
        key: "dicom.probe_extensions",
        title: "Extensions to check for DICOM",
        description: "Regular expression for the extensions (without the dot, empty for no extension, case-insensitive) of files whose first bytes are read to see if they are DICOM. .dcm, .dicom and .dic files are always shown as DICOM. The default checks files with no extension, only digits, dcm, dicom or ima.",
        widget: probe_extensions_widget,
    },
    Item {
        section: Section::DicomSend,
        key: "dicom_send.ae_title",
        title: "My AE title",
        description: "AE title vw uses when sending (Calling AE). Up to 16 of A-Z, 0-9, space, _ and -.",
        widget: ae_title_widget,
    },
    Item {
        section: Section::DicomSend,
        key: "dicom_send.timeout_secs",
        title: "Timeout",
        description: "Seconds to wait for a destination to connect and answer.",
        widget: send_timeout_widget,
    },
    Item {
        section: Section::Map,
        key: "map.enabled",
        title: "Use the map",
        description: "Show the map window (View > Map) with the places where photos were taken. The map pictures are downloaded from the server below while the window is open, so this is off until you turn it on.",
        widget: map_enabled_widget,
    },
    Item {
        section: Section::Map,
        key: "map.tile_url",
        title: "Map picture URL",
        description: "Where the map pictures (tiles) come from. {z}, {x} and {y} are replaced with the tile number. The default is the standard map of OpenStreetMap; follow the terms of the provider you use.",
        widget: map_tile_url_widget,
    },
    Item {
        section: Section::Map,
        key: "map.attribution",
        title: "Map credit",
        description: "Credit shown at the bottom right of the map. Map providers require it to be visible.",
        widget: map_attribution_widget,
    },
    Item {
        section: Section::Map,
        key: "map.attribution_url",
        title: "Map credit link",
        description: "Page opened by clicking the credit.",
        widget: map_attribution_url_widget,
    },
    Item {
        section: Section::Sky,
        key: "sky.enabled",
        title: "Use the sky chart",
        description: "Show the sky chart (View > Sky chart), which draws where astronomy images (FITS) point on a chart of the stars. It uses no network.",
        widget: sky_enabled_widget,
    },
    Item {
        section: Section::Sky,
        key: "sky.certain_color",
        title: "Frame color of known ranges",
        description: "Solid frame of images whose header has a WCS, so the range is known (#RRGGBB).",
        widget: sky_certain_color_widget,
    },
    Item {
        section: Section::Sky,
        key: "sky.approximate_color",
        title: "Frame color of estimated ranges",
        description: "Dotted frame of images with only the mount position (RA and DEC), sized from FOCALLEN and XPIXSZ with north up, and the cross of images without them (#RRGGBB).",
        widget: sky_approximate_color_widget,
    },
];

/// AE タイトルの入力欄。正しくない間は赤枠にして設定に入れない（保存もしない）。
/// 打っている途中の文字を設定とは別に持つのは、1 文字ごとに検査で弾くと、
/// 小文字を打ってから直すといった途中の状態を作れなくなるため。
fn ae_title_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    let id = ui.id().with("ae_title_draft");
    let mut draft = ui
        .data(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| s.dicom_send.ae_title.clone());
    let valid = vw_dicom::net::valid_ae_title(&draft);
    let r = ui.add(
        egui::TextEdit::singleline(&mut draft)
            .desired_width(160.0)
            .text_color_opt((!valid).then_some(Color32::from_rgb(230, 80, 80))),
    );
    if !valid {
        ui.painter().rect_stroke(
            r.rect,
            2.0,
            egui::Stroke::new(1.0, Color32::from_rgb(230, 80, 80)),
            egui::StrokeKind::Outside,
        );
    }
    let mut changed = false;
    if vw_dicom::net::valid_ae_title(&draft) && draft != s.dicom_send.ae_title {
        s.dicom_send.ae_title = draft.clone();
        changed = true;
    }
    if !r.has_focus() && vw_dicom::net::valid_ae_title(&draft) {
        draft = s.dicom_send.ae_title.clone();
    }
    ui.data_mut(|d| d.insert_temp(id, draft));
    changed
}

fn map_enabled_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.map.enabled, tr(lang, "Enabled"))
        .changed()
}

fn map_tile_url_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    checked_text(ui, "map.tile_url", &mut s.map.tile_url, 420.0, |t| {
        crate::map_tiles::valid_template(t)
    })
}

fn map_attribution_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    checked_text(ui, "map.attribution", &mut s.map.attribution, 320.0, |t| {
        !t.trim().is_empty()
    })
}

fn map_attribution_url_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    checked_text(
        ui,
        "map.attribution_url",
        &mut s.map.attribution_url,
        320.0,
        |t| t.starts_with("https://") || t.starts_with("http://"),
    )
}

/// 正しい値のときだけ設定に入れる入力欄（[`ae_title_widget`] と同じ考え方）。
fn checked_text(
    ui: &mut egui::Ui,
    key: &str,
    value: &mut String,
    width: f32,
    valid: impl Fn(&str) -> bool,
) -> bool {
    let id = ui.id().with((key, "draft"));
    let mut draft = ui
        .data(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| value.clone());
    let ok = valid(&draft);
    let red = Color32::from_rgb(230, 80, 80);
    let r = ui.add(
        egui::TextEdit::singleline(&mut draft)
            .desired_width(width)
            .text_color_opt((!ok).then_some(red)),
    );
    if !ok {
        ui.painter().rect_stroke(
            r.rect,
            2.0,
            egui::Stroke::new(1.0, red),
            egui::StrokeKind::Outside,
        );
    }
    let mut changed = false;
    if ok && draft != *value {
        *value = draft.clone();
        changed = true;
    }
    if !r.has_focus() && ok {
        draft = value.clone();
    }
    ui.data_mut(|d| d.insert_temp(id, draft));
    changed
}

fn send_timeout_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    use crate::settings::DicomSend;
    let r = ui.add(
        egui::DragValue::new(&mut s.dicom_send.timeout_secs)
            .range(DicomSend::TIMEOUT_RANGE)
            .suffix(" s"),
    );
    (r.changed() && !r.dragged()) || r.drag_stopped()
}

fn fullscreen_mode_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.fullscreen.mode;
    ui.horizontal(|ui| {
        for mode in FullscreenMode::ALL {
            ui.radio_value(&mut s.fullscreen.mode, mode, tr(lang, mode.title()));
        }
    });
    s.fullscreen.mode != before
}

fn fullscreen_sort_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.fullscreen.sort;
    vw_ui::menu::combo("fullscreen_sort")
        .selected_text(tr(lang, before.title()))
        .show_ui(ui, |ui| {
            for key in SortKey::ALL {
                ui.selectable_value(&mut s.fullscreen.sort, key, tr(lang, key.title()));
            }
        });
    s.fullscreen.sort != before
}

fn folders_descending_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.fullscreen.folders_descending, tr(lang, "Enabled"))
        .changed()
}

fn files_descending_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.fullscreen.files_descending, tr(lang, "Enabled"))
        .changed()
}

fn interval_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let r = ui.add(
        egui::DragValue::new(&mut s.fullscreen.interval_secs)
            .range(Fullscreen::INTERVAL_RANGE)
            .speed(0.1)
            .max_decimals(1)
            .suffix(tr(lang, " s")),
    );
    (r.changed() && !r.dragged()) || r.drag_stopped()
}

fn loop_at_end_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.fullscreen.loop_at_end, tr(lang, "Enabled"))
        .changed()
}

fn show_path_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.fullscreen.show_path, tr(lang, "Enabled"))
        .changed()
}

fn path_color_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    hex_color_widget(&mut s.fullscreen.path_color, Color32::GRAY, ui)
}

fn face_known_color_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    hex_color_widget(&mut s.ai.face_known_color, Color32::GREEN, ui)
}

fn face_unknown_color_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    hex_color_widget(&mut s.ai.face_unknown_color, Color32::YELLOW, ui)
}

fn sky_enabled_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.sky.enabled, tr(lang, "Enabled"))
        .changed()
}

fn sky_certain_color_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    hex_color_widget(&mut s.sky.certain_color, Color32::YELLOW, ui)
}

fn sky_approximate_color_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    hex_color_widget(&mut s.sky.approximate_color, Color32::RED, ui)
}

/// `#RRGGBB` の色を、色の見本のボタンと文字の欄で変える。
fn hex_color_widget(value: &mut String, fallback: Color32, ui: &mut egui::Ui) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut color = parse_hex_color(value).unwrap_or(fallback);
        if ui.color_edit_button_srgba(&mut color).changed() {
            *value = format!("#{:02x}{:02x}{:02x}", color.r(), color.g(), color.b());
            changed = true;
        }
        let r = ui.add(egui::TextEdit::singleline(value).desired_width(80.0));
        // 打ちかけの値（"#80" など）は保存しない。読み直したときに既定値へ戻されてしまうため。
        if r.changed() && parse_hex_color(value).is_some() {
            changed = true;
        }
    });
    changed
}

fn path_opacity_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    let r = ui.add(
        egui::DragValue::new(&mut s.fullscreen.path_opacity)
            .range(Fullscreen::OPACITY_RANGE)
            .suffix(" %"),
    );
    (r.changed() && !r.dragged()) || r.drag_stopped()
}

fn path_size_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    let r = ui.add(
        egui::DragValue::new(&mut s.fullscreen.path_size)
            .range(Fullscreen::PATH_SIZE_RANGE)
            .speed(0.2)
            .max_decimals(0),
    );
    (r.changed() && !r.dragged()) || r.drag_stopped()
}

/// 全画面スライドショーの準備の画面に、設定画面と同じ項目を並べる。変わったら true。
pub fn fullscreen_items_ui(ui: &mut egui::Ui, settings: &mut Settings) -> bool {
    let lang = settings.appearance.language;
    let mut changed = false;
    for item in ITEMS.iter().filter(|i| i.section == Section::Fullscreen) {
        item_frame(ui, |ui| {
            title_line(ui, tr(lang, item.title), item.key);
            ui.weak(tr(lang, item.description));
            changed |= (item.widget)(settings, ui, lang);
        });
    }
    changed
}

/// つないでいるモニタと、設定にだけあるモニタ（今は外れている）を並べ、10bit とディザを切り替える。
fn monitors_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let system = crate::displays::cached();
    let surface = crate::deep::surface();
    if system.displays.is_empty() {
        ui.weak(tr(lang, "The monitors cannot be listed here."));
    }
    let mut rows: Vec<(String, String, MonitorSettings)> = system
        .displays
        .iter()
        .map(|d| {
            let mut m = s.display.monitor(&d.id);
            m.name.clone_from(&d.name);
            (d.label(), crate::displays::signal_text(d, lang), m)
        })
        .collect();
    rows.extend(
        s.display
            .monitors
            .iter()
            .filter(|m| !system.displays.iter().any(|d| d.id == m.id))
            .map(|m| {
                let label = if m.name.is_empty() { &m.id } else { &m.name };
                (
                    label.clone(),
                    tr(lang, "(not connected)").to_owned(),
                    m.clone(),
                )
            }),
    );
    let mut changed = false;
    for (label, detail, mut m) in rows {
        ui.horizontal(|ui| {
            let before = (m.ten_bit, m.dither);
            ui.checkbox(&mut m.ten_bit, tr(lang, "10-bit"));
            ui.checkbox(&mut m.dither, tr(lang, "Dithering"));
            ui.label(label);
            if !detail.is_empty() {
                ui.weak(detail);
            }
            if (m.ten_bit, m.dither) != before {
                s.display.set(m);
                changed = true;
            }
        });
    }
    let started = if surface.bits >= 10 {
        tr1(
            lang,
            "This start: {}-bit surface.",
            &surface.bits.to_string(),
        )
    } else if s.display.wants_ten_bit() {
        tr(lang, "This start: 8-bit surface. Restart vw to try 10-bit; if it still says 8-bit, see Help > System information.").to_owned()
    } else {
        tr(lang, "This start: 8-bit surface.").to_owned()
    };
    ui.weak(started);
    changed
}

fn theme_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.appearance.theme;
    ui.horizontal(|ui| {
        for (choice, label) in [
            (ThemeChoice::System, "System"),
            (ThemeChoice::Dark, "Dark"),
            (ThemeChoice::Light, "Light"),
        ] {
            ui.radio_value(&mut s.appearance.theme, choice, tr(lang, label));
        }
    });
    s.appearance.theme != before
}

fn language_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    let before = s.appearance.language;
    vw_ui::menu::combo("language")
        .selected_text(before.native_name())
        .show_ui(ui, |ui| {
            for l in Language::ALL {
                ui.selectable_value(&mut s.appearance.language, l, l.native_name());
            }
        });
    s.appearance.language != before
}

fn scale_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    use crate::settings::Appearance;
    let r = ui.add(
        egui::DragValue::new(&mut s.appearance.ui_scale)
            .range(Appearance::MIN_SCALE..=Appearance::MAX_SCALE)
            .speed(0.01)
            .fixed_decimals(2),
    );
    // ドラッグ中は反映しない。倍率が変わるとつまんでいる部品自体が動いてしまうため、離したときに反映する。
    (r.changed() && !r.dragged()) || r.drag_stopped()
}

fn compact_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.appearance.compact, tr(lang, "Enabled"))
        .changed()
}

fn limit_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    let r = ui.add(
        egui::DragValue::new(&mut s.viewer.full_image_limit_mb)
            .range(Viewer::LIMIT_RANGE_MB)
            .speed(64.0)
            .suffix(" MB"),
    );
    // ドラッグ中に反映すると、値が下がるたびに画像を捨てて読み直してしまうため、離したときに反映する。
    (r.changed() && !r.dragged()) || r.drag_stopped()
}

fn image_size_limit_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    let r = ui.add(
        egui::DragValue::new(&mut s.viewer.image_size_limit_mb)
            .range(Viewer::IMAGE_SIZE_RANGE_MB)
            .speed(64.0)
            .suffix(" MB"),
    );
    (r.changed() && !r.dragged()) || r.drag_stopped()
}

/// レイアウトの選択肢に出す名前。
pub fn layout_title(layout: SlotLayout, lang: Language) -> String {
    match layout {
        SlotLayout::Auto => tr(lang, "Auto").to_owned(),
        SlotLayout::Grid { cols, rows } => format!("{cols} x {rows}"),
    }
}

fn layout_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.viewer.layout;
    let custom = s.viewer.custom_layout;
    vw_ui::menu::combo("viewer_layout")
        .selected_text(layout_title(before, lang))
        .show_ui(ui, |ui| {
            for layout in SlotLayout::PRESETS {
                ui.selectable_value(&mut s.viewer.layout, layout, layout_title(layout, lang));
            }
            if !SlotLayout::PRESETS.contains(&custom) {
                let text = format!("{} ({})", tr(lang, "Custom"), layout_title(custom, lang));
                ui.selectable_value(&mut s.viewer.layout, custom, text);
            }
        });
    s.viewer.layout != before
}

/// 列数と行数の入力欄。変えたら `true`。
fn grid_input(layout: &mut SlotLayout, ui: &mut egui::Ui) -> bool {
    let SlotLayout::Grid { mut cols, mut rows } = *layout else {
        return false;
    };
    let side = 1..=SlotLayout::MAX_SIDE;
    let changed = ui
        .horizontal(|ui| {
            let c = ui.add(egui::DragValue::new(&mut cols).range(side.clone()));
            ui.label("x");
            let r = ui.add(egui::DragValue::new(&mut rows).range(side));
            c.changed() || r.changed()
        })
        .inner;
    if changed {
        *layout = SlotLayout::grid(cols, rows);
    }
    changed
}

fn auto_layout_max_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    grid_input(&mut s.viewer.auto_layout_max, ui)
}

fn custom_layout_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    let old = s.viewer.custom_layout;
    if !grid_input(&mut s.viewer.custom_layout, ui) {
        return false;
    }
    // カスタムを選んでいる間は、マス目を変えたらすぐ画面に反映する。
    if s.viewer.layout == old {
        s.viewer.layout = s.viewer.custom_layout;
    }
    true
}

/// スライドショーの FPS の入力欄。再生バーと設定画面で同じものを使う。
pub fn fps_input(fps: &mut f32) -> egui::DragValue<'_> {
    egui::DragValue::new(fps)
        .range(FPS_RANGE)
        .speed(0.2)
        .max_decimals(1)
        .suffix(" fps")
}

fn fps_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    ui.add(fps_input(&mut s.viewer.slideshow_fps)).changed()
}

fn drop_window_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.add(
        egui::DragValue::new(&mut s.viewer.slideshow_drop_window_secs)
            .range(DROP_WINDOW_RANGE)
            .suffix(tr(lang, " s")),
    )
    .changed()
}

fn page_wheel_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.viewer.page_wheel;
    ui.horizontal(|ui| {
        for unit in WheelScroll::ALL {
            ui.radio_value(&mut s.viewer.page_wheel, unit, tr(lang, unit.title()));
        }
    });
    s.viewer.page_wheel != before
}

fn end_mode_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.viewer.slideshow_end;
    ui.horizontal(|ui| {
        for mode in EndMode::ALL {
            ui.radio_value(&mut s.viewer.slideshow_end, mode, tr(lang, mode.title()));
        }
    });
    s.viewer.slideshow_end != before
}

fn load_into_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.tree.load_into;
    ui.horizontal(|ui| {
        for (choice, label) in [
            (LoadInto::Images, "One slot per image"),
            (LoadInto::Folders, "One slot per folder"),
        ] {
            ui.radio_value(&mut s.tree.load_into, choice, tr(lang, label));
        }
    });
    s.tree.load_into != before
}

fn other_load_modifier_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.tree.other_load_modifier;
    ui.horizontal(|ui| {
        for (choice, label) in [
            (LoadModifier::Alt, "Alt"),
            (LoadModifier::Ctrl, "Ctrl"),
            (LoadModifier::None, tr(lang, "None")),
        ] {
            ui.radio_value(&mut s.tree.other_load_modifier, choice, label);
        }
    });
    s.tree.other_load_modifier != before
}

fn sort_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.tree.sort;
    vw_ui::menu::combo("tree_sort")
        .selected_text(tr(lang, before.title()))
        .show_ui(ui, |ui| {
            for key in SortKey::ALL {
                ui.selectable_value(&mut s.tree.sort, key, tr(lang, key.title()));
            }
        });
    s.tree.sort != before
}

fn descending_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.tree.sort_descending, tr(lang, "Enabled"))
        .changed()
}

fn confirm_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.tree.confirm_every, tr(lang, "Enabled"))
        .changed()
}

fn click_expands_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.tree.click_expands, tr(lang, "Enabled"))
        .changed()
}

fn chunk_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let r = ui.add_enabled(
        s.tree.confirm_every,
        egui::DragValue::new(&mut s.tree.confirm_chunk)
            .range(Tree::CHUNK_RANGE)
            .speed(10.0)
            .suffix(tr(lang, " images")),
    );
    (r.changed() && !r.dragged()) || r.drag_stopped()
}

fn keep_view_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.viewer.keep_view, tr(lang, "Enabled"))
        .changed()
}

fn magnify_filter_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    scale_filter_radios(&mut s.viewer.magnify_filter, ui, lang)
}

fn minify_filter_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    scale_filter_radios(&mut s.viewer.minify_filter, ui, lang)
}

fn scale_filter_radios(value: &mut ScaleFilter, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = *value;
    ui.horizontal(|ui| {
        for filter in ScaleFilter::ALL {
            ui.radio_value(value, filter, tr(lang, filter.title()));
        }
    });
    *value != before
}

fn fits_stretch_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.fits.stretch;
    ui.horizontal(|ui| {
        for stretch in FitsStretch::ALL {
            ui.radio_value(&mut s.fits.stretch, stretch, tr(lang, stretch.title()));
        }
    });
    s.fits.stretch != before
}

fn pipeline_color_node_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(
        &mut s.pipeline.color_node,
        tr(lang, "Insert from the start"),
    )
    .changed()
}

/// 「初めから入れる」と、その表。表の一覧は組み込みと自分で足した表。
fn color_node_widget(
    node: &mut ColorNode,
    custom: &[CustomPalette],
    id: &str,
    ui: &mut egui::Ui,
    lang: Language,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        changed |= ui
            .checkbox(&mut node.color_node, tr(lang, "Insert from the start"))
            .changed();
        let names = vw_core::palette::BUILTIN
            .iter()
            .map(|n| (*n).to_owned())
            .chain(custom.iter().map(|c| c.name.clone()));
        let title = crate::colormap_ui::palette_title(lang, &node.palette);
        vw_ui::menu::combo(id)
            .selected_text(title)
            .show_ui(ui, |ui| {
                for name in names {
                    let title = crate::colormap_ui::palette_title(lang, &name);
                    if ui.selectable_label(node.palette == name, title).clicked() {
                        node.palette = name;
                        changed = true;
                    }
                }
            });
    });
    changed
}

fn pipeline_fits_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let p = &mut s.pipeline;
    color_node_widget(&mut p.fits, &p.palettes, "pipeline.fits", ui, lang)
}

fn pipeline_dicom_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let p = &mut s.pipeline;
    let mut node = ColorNode {
        color_node: p.dicom.color_node,
        palette: p.dicom.palette.clone(),
    };
    let changed = color_node_widget(&mut node, &p.palettes, "pipeline.dicom", ui, lang);
    p.dicom.color_node = node.color_node;
    p.dicom.palette = node.palette;
    changed
}

fn pipeline_pt_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let p = &mut s.pipeline;
    color_node_widget(&mut p.dicom.pt, &p.palettes, "pipeline.dicom.pt", ui, lang)
}

fn pipeline_gray_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let p = &mut s.pipeline;
    color_node_widget(&mut p.gray, &p.palettes, "pipeline.gray", ui, lang)
}

fn pipeline_gauge_side_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.pipeline.gauge_side;
    ui.horizontal(|ui| {
        for side in GaugeSide::ALL {
            ui.radio_value(&mut s.pipeline.gauge_side, side, tr(lang, side.title()));
        }
    });
    s.pipeline.gauge_side != before
}

fn pipeline_gauge_gray_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.pipeline.gauge_on_gray, tr(lang, "Enabled"))
        .changed()
}

/// 1 行に 1 つの表。読めない行や同じ名前があれば知らせ、直すまで書き戻さない。
/// 消した表を使っていた種類は、読み直したときに既定の表に戻る（設定の読み取りと同じ）。
fn pipeline_palettes_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let list = &mut s.pipeline.palettes;
    let id = ui.make_persistent_id("pipeline.palettes.edit");
    let mut text: String = ui.data_mut(|d| d.get_temp(id)).unwrap_or_else(|| {
        list.iter()
            .map(CustomPalette::to_line)
            .collect::<Vec<_>>()
            .join("\n")
    });
    let r = ui.add(
        egui::TextEdit::multiline(&mut text)
            .hint_text("Ice = 0 #000000; 0.5 #0080ff; 1 #ffffff")
            .desired_rows(3)
            .desired_width(420.0),
    );
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let parsed: Vec<CustomPalette> = lines
        .iter()
        .filter_map(|l| CustomPalette::parse_line(l))
        .collect();
    let mut names = std::collections::HashSet::new();
    let unique = parsed.iter().all(|c| names.insert(c.name.as_str()));
    let valid = parsed.len() == lines.len() && unique;
    if !valid {
        ui.colored_label(
            ui.visuals().error_fg_color,
            tr(
                lang,
                "Write each line as name = position #rrggbb; position #rrggbb, with a different name that is not a built-in color map.",
            ),
        );
    }
    let mut changed = false;
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        if r.lost_focus() && valid && parsed != *list {
            *list = parsed;
            changed = true;
        }
        ui.data_mut(|d| d.remove::<String>(id));
    }
    changed
}

fn fits_curve_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.fits.curve;
    ui.horizontal(|ui| {
        for curve in FitsCurve::ALL {
            ui.radio_value(&mut s.fits.curve, curve, tr(lang, curve.title()));
        }
    });
    s.fits.curve != before
}

/// 黒は白より小さく、白は黒より大きくしか動かせない。
fn fits_low_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    let high = s.fits.high_percent;
    percent_widget(&mut s.fits.low_percent, 0.0..=(high - 0.1).max(0.0), ui)
}

fn fits_high_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    let low = s.fits.low_percent;
    percent_widget(&mut s.fits.high_percent, (low + 0.1).min(100.0)..=100.0, ui)
}

fn percent_widget(v: &mut f32, range: std::ops::RangeInclusive<f32>, ui: &mut egui::Ui) -> bool {
    let r = ui.add(
        egui::DragValue::new(v)
            .range(range)
            .speed(0.1)
            .fixed_decimals(1)
            .suffix(" %"),
    );
    (r.changed() && !r.dragged()) || r.drag_stopped()
}

fn fits_flip_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.fits.flip_rows, tr(lang, "Enabled"))
        .changed()
}

fn fits_demosaic_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.fits.demosaic, tr(lang, "Enabled"))
        .changed()
}

fn calib_apply_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let c = &mut s.fits.calibration;
    let before = c.apply;
    ui.horizontal(|ui| {
        ui.radio_value(&mut c.apply, CalibrationApply::Auto, tr(lang, "When found"));
        ui.radio_value(&mut c.apply, CalibrationApply::Off, tr(lang, "Off"));
    });
    c.apply != before
}

fn calib_nearby_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.fits.calibration.search_nearby, tr(lang, "Enabled"))
        .changed()
}

fn calib_library_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    let hint = if cfg!(windows) {
        "D:\\astro\\masters"
    } else {
        "/data/astro/masters"
    };
    folder_list_widget(
        &mut s.fits.calibration.library,
        "fits.calibration.library",
        hint,
        ui,
    )
}

fn calib_use_dark_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.fits.calibration.use_dark, tr(lang, "Enabled"))
        .changed()
}

fn calib_use_flat_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.fits.calibration.use_flat, tr(lang, "Enabled"))
        .changed()
}

fn calib_use_bias_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.fits.calibration.use_bias, tr(lang, "Enabled"))
        .changed()
}

fn calib_exposure_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    tolerance_widget(&mut s.fits.calibration.exposure_tolerance_secs, " s", ui)
}

fn calib_temperature_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    tolerance_widget(&mut s.fits.calibration.temperature_tolerance, " °C", ui)
}

fn tolerance_widget(v: &mut f64, suffix: &str, ui: &mut egui::Ui) -> bool {
    let r = ui.add(
        egui::DragValue::new(v)
            .range(FitsCalibration::TOLERANCE_RANGE)
            .speed(0.1)
            .fixed_decimals(1)
            .prefix("± ")
            .suffix(suffix),
    );
    (r.changed() && !r.dragged()) || r.drag_stopped()
}

fn calib_gain_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(
        &mut s.fits.calibration.match_gain_offset,
        tr(lang, "Enabled"),
    )
    .changed()
}

fn calib_scaling_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let c = &mut s.fits.calibration;
    let before = c.dark_scaling;
    ui.horizontal(|ui| {
        ui.radio_value(
            &mut c.dark_scaling,
            DarkScaling::None,
            tr(lang, "Same exposure only"),
        );
        ui.radio_value(
            &mut c.dark_scaling,
            DarkScaling::Exposure,
            tr(lang, "Scale by exposure"),
        );
    });
    c.dark_scaling != before
}

fn calib_flat_match_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let c = &mut s.fits.calibration;
    let before = c.flat_match;
    ui.horizontal(|ui| {
        ui.radio_value(
            &mut c.flat_match,
            FlatMatch::Filter,
            tr(lang, "Same filter"),
        );
        ui.radio_value(
            &mut c.flat_match,
            FlatMatch::FilterAndNight,
            tr(lang, "Same filter and night"),
        );
    });
    c.flat_match != before
}

fn calib_normalize_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let c = &mut s.fits.calibration;
    let before = c.flat_normalize;
    ui.horizontal(|ui| {
        ui.radio_value(
            &mut c.flat_normalize,
            FlatNormalize::Median,
            tr(lang, "Median value"),
        );
        ui.radio_value(
            &mut c.flat_normalize,
            FlatNormalize::Mean,
            tr(lang, "Mean value"),
        );
    });
    c.flat_normalize != before
}

fn calib_equalize_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.fits.calibration.equalize_cfa, tr(lang, "Enabled"))
        .changed()
}

/// 数でないあいだは赤くして、数か空になったときだけ設定にする。
fn calib_synthetic_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    let id = ui.id().with("synthetic_bias_draft");
    let current = &mut s.fits.calibration.synthetic_bias;
    let mut draft = ui
        .data(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| current.clone());
    let valid = FitsCalibration::valid_bias(&draft);
    let r = ui.add(
        egui::TextEdit::singleline(&mut draft)
            .hint_text("2048")
            .desired_width(100.0)
            .text_color_opt((!valid).then_some(Color32::from_rgb(230, 80, 80))),
    );
    let mut changed = false;
    if FitsCalibration::valid_bias(&draft) && draft.trim() != current {
        *current = draft.trim().to_owned();
        changed = true;
    }
    if !r.has_focus() && FitsCalibration::valid_bias(&draft) {
        draft = current.clone();
    }
    ui.data_mut(|d| d.insert_temp(id, draft));
    changed
}

fn develop_raw_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.viewer.develop_raw, tr(lang, "Enabled"))
        .changed()
}

fn log_style_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.log.style;
    ui.horizontal(|ui| {
        for style in LogStyle::ALL {
            ui.radio_value(&mut s.log.style, style, tr(lang, style.title()));
        }
    });
    s.log.style != before
}

fn log_file_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.log.file, tr(lang, "Enabled")).changed()
}

fn log_keep_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    let r = ui.add(egui::DragValue::new(&mut s.log.keep_files).range(LogSettings::KEEP_RANGE));
    (r.changed() && !r.dragged()) || r.drag_stopped()
}

fn log_wrap_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.log.wrap, tr(lang, "Enabled")).changed()
}

fn ai_general_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    unit_widget(&mut s.ai.general_threshold, ui)
}

fn ai_face_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    unit_widget(&mut s.ai.face_threshold, ui)
}

fn ai_character_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    unit_widget(&mut s.ai.character_threshold, ui)
}

fn unit_widget(v: &mut f32, ui: &mut egui::Ui) -> bool {
    let r = ui.add(
        egui::DragValue::new(v)
            .range(0.0..=1.0)
            .speed(0.01)
            .fixed_decimals(2),
    );
    (r.changed() && !r.dragged()) || r.drag_stopped()
}

/// 1 行に 1 つ。タグ名にはカンマを含むものがあるので、カンマ区切りにはしない。
fn ai_excluded_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    let id = ui.make_persistent_id("ai.excluded_tags.edit");
    let mut text: String = ui
        .data_mut(|d| d.get_temp(id))
        .unwrap_or_else(|| s.ai.excluded_tags.join("\n"));
    let r = ui.add(
        egui::TextEdit::multiline(&mut text)
            .hint_text("watermark")
            .desired_rows(3)
            .desired_width(320.0),
    );
    let parsed: Vec<String> = text
        .lines()
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect();
    let mut changed = false;
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        if r.lost_focus() && parsed != s.ai.excluded_tags {
            s.ai.excluded_tags = parsed;
            changed = true;
        }
        ui.data_mut(|d| d.remove::<String>(id));
    }
    changed
}

fn ai_threads_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    let r =
        ui.add(egui::DragValue::new(&mut s.ai.threads).range(crate::settings::Ai::THREADS_RANGE));
    (r.changed() && !r.dragged()) || r.drag_stopped()
}

fn quality_psnr_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    thresholds_widget(&mut s.quality.psnr, ui, lang, 1.0, 1)
}

fn quality_ssim_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    thresholds_widget(&mut s.quality.ssim, ui, lang, 0.01, 2)
}

fn quality_snr_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    thresholds_widget(&mut s.quality.snr, ui, lang, 0.1, 1)
}

fn quality_cnr_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    thresholds_widget(&mut s.quality.cnr, ui, lang, 0.1, 1)
}

fn quality_sharpness_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    thresholds_widget(&mut s.quality.sharpness, ui, lang, 10.0, 0)
}

/// 「適合」と「注意」の下限。適合の下限が注意の下限を下回るような途中の値は保存しない。
fn thresholds_widget(
    t: &mut Thresholds,
    ui: &mut egui::Ui,
    lang: Language,
    speed: f64,
    decimals: usize,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        for (label, value) in [("Good from", &mut t.good), ("Caution from", &mut t.caution)] {
            ui.label(tr(lang, label));
            let r = ui.add(
                egui::DragValue::new(value)
                    .speed(speed)
                    .fixed_decimals(decimals),
            );
            changed |= (r.changed() && !r.dragged()) || r.drag_stopped();
        }
        ui.weak(tr(lang, "Below caution is degraded."));
    });
    changed && t.valid()
}

fn auto_reload_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.behavior.auto_reload, tr(lang, "Enabled"))
        .changed()
}

/// 展開先のフォルダ。入力中の文字列は egui の一時データに持ち、確定したときに反映する。
/// 1 文字ごとに反映すると、打っている途中のパスごとに展開先が変わり、ログも流れてしまうため。
fn archive_size_limit_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    let r = ui.add(
        egui::DragValue::new(&mut s.behavior.archive_size_limit_gb)
            .range(crate::settings::Behavior::ARCHIVE_SIZE_RANGE_GB)
            .speed(1.0)
            .suffix(" GB"),
    );
    (r.changed() && !r.dragged()) || r.drag_stopped()
}

fn archive_dir_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let id = ui.make_persistent_id("behavior.archive_dir.edit");
    let picker_id = id.with("picker");
    let mut text: String = ui
        .data_mut(|d| d.get_temp(id))
        .unwrap_or_else(|| s.behavior.archive_dir.clone());
    let mut changed = false;
    ui.horizontal(|ui| {
        let r = ui.add(
            egui::TextEdit::singleline(&mut text)
                .hint_text(std::env::temp_dir().display().to_string())
                .desired_width(360.0),
        );
        if ui.button(tr(lang, "Choose...")).clicked() {
            let start = s.behavior.archive_parent();
            let picker = vw_tree::picker::Picker::folder(
                tr(lang, "Archive extraction folder"),
                start.as_deref(),
                picker_labels(lang),
            );
            ui.data_mut(|d| d.insert_temp(picker_id, Arc::new(Mutex::new(picker))));
        }
        let valid = crate::settings::Behavior::valid_archive_dir(&text);
        if !valid {
            ui.colored_label(
                ui.visuals().error_fg_color,
                tr(lang, "Use an absolute path."),
            );
        }
        if r.has_focus() {
            ui.data_mut(|d| d.insert_temp(id, text.clone()));
        } else {
            let text = text.trim();
            if r.lost_focus() && valid && text != s.behavior.archive_dir {
                s.behavior.archive_dir = text.to_owned();
                changed = true;
            }
            ui.data_mut(|d| d.remove::<String>(id));
        }
    });
    let picker: Option<Arc<Mutex<vw_tree::picker::Picker>>> =
        ui.data_mut(|d| d.get_temp(picker_id));
    if let Some(picker) = picker {
        let outcome = picker.lock().unwrap().show(ui.ctx());
        match outcome {
            vw_tree::picker::Outcome::Open => {}
            vw_tree::picker::Outcome::Chosen(dir) => {
                s.behavior.archive_dir = dir.display().to_string();
                changed = true;
                ui.data_mut(|d| d.remove::<Arc<Mutex<vw_tree::picker::Picker>>>(picker_id));
            }
            vw_tree::picker::Outcome::Cancelled => {
                ui.data_mut(|d| d.remove::<Arc<Mutex<vw_tree::picker::Picker>>>(picker_id));
            }
        }
    }
    changed
}

pub fn picker_labels(lang: Language) -> vw_tree::picker::Labels {
    vw_tree::picker::Labels {
        choose: tr(lang, "Choose").into(),
        cancel: tr(lang, "Cancel").into(),
        name: tr(lang, "Name").into(),
        nothing: tr(lang, "(no folder chosen)").into(),
    }
}

/// タグの一覧をカンマ区切りで編集する。入力中の文字列は egui の一時データに持ち、
/// 確定（Enter かフォーカスを外す）したときに正しい一覧なら反映する。1 文字ごとに反映すると、
/// 「cat,」と打った途中で空のタグができて、そのたびに不正な値として扱うことになるため。
fn tags_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let id = ui.make_persistent_id("labels.tags.edit");
    let joined = s.labels.tags.join(", ");
    let mut text: String = ui
        .data_mut(|d| d.get_temp(id))
        .unwrap_or_else(|| joined.clone());
    let r = ui.add(
        egui::TextEdit::singleline(&mut text)
            .hint_text("cat, dog, bird")
            .desired_width(320.0),
    );
    let parsed: Vec<String> = text
        .split(',')
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .collect();
    let valid = Labels::valid_tags(&parsed);
    if !valid {
        ui.colored_label(
            ui.visuals().error_fg_color,
            tr(lang, "Tag names must be unique."),
        );
    }
    let mut changed = false;
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        if r.lost_focus() && valid && parsed != s.labels.tags {
            s.labels.tags = parsed;
            changed = true;
        }
        ui.data_mut(|d| d.remove::<String>(id));
    }
    changed
}

fn dataset_roots_widget(s: &mut Settings, ui: &mut egui::Ui, _lang: Language) -> bool {
    let hint = if cfg!(windows) {
        "D:\\datasets"
    } else {
        "/data/datasets"
    };
    folder_list_widget(&mut s.dataset.roots, "dataset.roots", hint, ui)
}

/// 1 行に 1 つのフォルダ。打っているあいだは反映せず、欄を離れたときに設定にする。
fn folder_list_widget(list: &mut Vec<String>, key: &str, hint: &str, ui: &mut egui::Ui) -> bool {
    let id = ui.make_persistent_id((key, "edit"));
    let mut text: String = ui
        .data_mut(|d| d.get_temp(id))
        .unwrap_or_else(|| list.join("\n"));
    let r = ui.add(
        egui::TextEdit::multiline(&mut text)
            .hint_text(hint)
            .desired_rows(3)
            .desired_width(420.0),
    );
    let parsed: Vec<String> = text
        .lines()
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect();
    let mut changed = false;
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        if r.lost_focus() && parsed != *list {
            *list = parsed;
            changed = true;
        }
        ui.data_mut(|d| d.remove::<String>(id));
    }
    changed
}

fn search_groups_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    groups_editor(
        ui,
        lang,
        "dataset.search_groups.edit",
        "Patient = PatientID; PatientName",
        &mut s.dataset.search_groups,
    )
}

fn metadata_groups_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    groups_editor(
        ui,
        lang,
        "metadata.groups.edit",
        "Camera = EXIF/Model; EXIF/LensModel",
        &mut s.metadata.groups,
    )
}

/// 1 行に 1 組。読めない行があるうちは反映しない（打っている途中の行で組が消えないように）。
fn groups_editor(
    ui: &mut egui::Ui,
    lang: Language,
    id: &str,
    hint: &str,
    groups: &mut Vec<SearchGroup>,
) -> bool {
    let id = ui.make_persistent_id(id);
    let mut text: String = ui.data_mut(|d| d.get_temp(id)).unwrap_or_else(|| {
        groups
            .iter()
            .map(SearchGroup::to_line)
            .collect::<Vec<_>>()
            .join("\n")
    });
    let r = ui.add(
        egui::TextEdit::multiline(&mut text)
            .hint_text(hint)
            .desired_rows(4)
            .desired_width(420.0),
    );
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let parsed: Vec<SearchGroup> = lines
        .iter()
        .filter_map(|l| SearchGroup::parse_line(l))
        .collect();
    let mut names = std::collections::HashSet::new();
    let unique = parsed.iter().all(|g| names.insert(g.name.as_str()));
    let valid = parsed.len() == lines.len() && unique;
    if !valid {
        ui.colored_label(
            ui.visuals().error_fg_color,
            tr(
                lang,
                "Write each line as name = key; key, with a different name on each line.",
            ),
        );
    }
    let mut changed = false;
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        if r.lost_focus() && valid && parsed != *groups {
            *groups = parsed;
            changed = true;
        }
        ui.data_mut(|d| d.remove::<String>(id));
    }
    changed
}

/// 隠した項目を 1 行に 1 つ並べ、✕ で元に戻す。足すのはメタデータ欄の右クリックから。
fn hidden_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    if s.metadata.hidden.is_empty() {
        ui.weak(tr(lang, "Nothing is hidden."));
        return false;
    }
    let mut remove = None;
    for (i, id) in s.metadata.hidden.iter().enumerate() {
        ui.horizontal(|ui| {
            if ui
                .small_button(icon::X)
                .on_hover_text(tr(lang, "Show it again"))
                .clicked()
            {
                remove = Some(i);
            }
            ui.label(id);
        });
    }
    if let Some(i) = remove {
        s.metadata.hidden.remove(i);
    }
    remove.is_some()
}

fn heading_style_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.metadata.heading_style;
    ui.horizontal(|ui| {
        for (choice, label) in [
            (HeadingStyle::Color, "Accent color"),
            (HeadingStyle::Underline, "Underline"),
            (HeadingStyle::Both, "Color and underline"),
            (HeadingStyle::Plain, "Plain"),
        ] {
            ui.radio_value(&mut s.metadata.heading_style, choice, tr(lang, label));
        }
    });
    s.metadata.heading_style != before
}

fn remember_open_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.metadata.remember_open, tr(lang, "Remember"))
        .changed()
}

fn probe_color_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    color_format_radio(ui, lang, &mut s.metadata.color_format)
}

fn roi_color_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    color_format_radio(ui, lang, &mut s.roi.color_format)
}

fn color_format_radio(ui: &mut egui::Ui, lang: Language, value: &mut ColorFormat) -> bool {
    let before = *value;
    ui.horizontal(|ui| {
        for (choice, label) in [
            (ColorFormat::Rgb, "RGB numbers (R 128 G 64 B 255)"),
            (ColorFormat::Hex, "Color code (#8040FF)"),
        ] {
            ui.radio_value(value, choice, tr(lang, label));
        }
    });
    *value != before
}

/// タグと同じくカンマ区切りで編集する。
fn dataset_fields_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let id = ui.make_persistent_id("dataset.fields.edit");
    let mut text: String = ui
        .data_mut(|d| d.get_temp(id))
        .unwrap_or_else(|| s.dataset.fields.join(", "));
    let r = ui.add(
        egui::TextEdit::singleline(&mut text)
            .hint_text("Source, Purpose")
            .desired_width(320.0),
    );
    let parsed: Vec<String> = text
        .split(',')
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .collect();
    let valid = Labels::valid_tags(&parsed);
    if !valid {
        ui.colored_label(
            ui.visuals().error_fg_color,
            tr(lang, "Field names must be unique."),
        );
    }
    let mut changed = false;
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        if r.lost_focus() && valid && parsed != s.dataset.fields {
            s.dataset.fields = parsed;
            changed = true;
        }
        ui.data_mut(|d| d.remove::<String>(id));
    }
    changed
}

/// 拡張子の正規表現を編集する。タグと同じく確定したときに反映する。1 文字ごとに反映すると、
/// 打っている途中の正しくない式のたびにフォルダを読み直すことになるため。
fn probe_extensions_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let id = ui.make_persistent_id("dicom.probe_extensions.edit");
    let mut text: String = ui
        .data_mut(|d| d.get_temp(id))
        .unwrap_or_else(|| s.dicom.probe_extensions.clone());
    let r = ui.add(
        egui::TextEdit::singleline(&mut text)
            .font(egui::TextStyle::Monospace)
            .desired_width(320.0),
    );
    let error = vw_core::ProbeExtensions::new(&text).err();
    if let Some(e) = &error {
        let first = e.lines().last().unwrap_or_default();
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("{}: {first}", tr(lang, "Not a valid regular expression")),
        );
    }
    let mut changed = false;
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        if r.lost_focus() && error.is_none() && text != s.dicom.probe_extensions {
            s.dicom.probe_extensions = text;
            changed = true;
        }
        ui.data_mut(|d| d.remove::<String>(id));
    }
    changed
}

fn advance_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    ui.checkbox(&mut s.labels.advance, tr(lang, "Enabled"))
        .changed()
}

fn header_click_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.behavior.header_click;
    ui.horizontal(|ui| {
        for (choice, label) in [
            (HeaderClick::Select, "Select only"),
            (HeaderClick::SelectAndToggle, "Select and expand/collapse"),
        ] {
            ui.radio_value(&mut s.behavior.header_click, choice, tr(lang, label));
        }
    });
    s.behavior.header_click != before
}

fn path_display_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let before = s.metadata.path;
    ui.horizontal(|ui| {
        for (choice, label) in [
            (PathDisplay::Full, "Full path"),
            (PathDisplay::Name, "File name only"),
        ] {
            ui.radio_value(&mut s.metadata.path, choice, tr(lang, label));
        }
    });
    s.metadata.path != before
}

fn background_widget(s: &mut Settings, ui: &mut egui::Ui, lang: Language) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        changed = ui
            .add(
                egui::TextEdit::singleline(&mut s.viewer.background)
                    .hint_text(tr(lang, "Theme default"))
                    .desired_width(110.0),
            )
            .changed();
        let text = s.viewer.background.trim();
        match parse_hex_color(text) {
            Some(c) => {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 0.0, c);
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    ui.visuals().widgets.noninteractive.bg_stroke,
                    egui::StrokeKind::Inside,
                );
            }
            None if !text.is_empty() => {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    tr(lang, "Use the #RRGGBB format."),
                );
            }
            None => {}
        }
    });
    changed
}

/// `query` の語（空白区切り）がすべて `texts` のどれかに含まれるか。大文字小文字は区別しない。
pub fn matches(query: &str, texts: &[&str]) -> bool {
    let haystack = texts.join("\n").to_lowercase();
    query
        .split_whitespace()
        .all(|word| haystack.contains(&word.to_lowercase()))
}

/// キー割り当ての操作の結果。該当する操作の行の下に出す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Notice {
    Refused(Refusal),
    /// 割り当てたが、OS がそのキーを使っていることが多い。
    OsReserved(&'static str),
    Clipboard,
}

/// 設定画面の 1 フレーム分の結果。
#[derive(Default)]
pub struct Response {
    /// 設定が変わった。反映と保存が要る。
    pub changed: bool,
    pub open_file: bool,
    pub open_log_folder: bool,
    pub close: bool,
    /// ログに出すこと（接続確認の結果）。`true` なら成功。
    pub logs: Vec<(bool, String)>,
}

/// 設定画面の状態。
#[derive(Default)]
pub struct SettingsUi {
    query: String,
    /// キー入力を待っている操作。
    capturing: Option<Action>,
    notice: Option<(Action, Notice)>,
    jump: Option<Section>,
    /// キーの割り当ての中で、目次から飛ぶまとまり。
    key_jump: Option<KeyGroup>,
    /// キーの割り当ての中で、いま見ているまとまり。節と同じく前のフレームの位置から決める。
    current_group: Option<KeyGroup>,
    seen_group: Option<KeyGroup>,
    /// 目次で強調する、いま見ている節。前のフレームで描いた位置から決める。
    current: Option<Section>,
    /// 描いている途中のフレームで、見ている節の候補。
    seen: Option<Section>,
    focus_search: bool,
    destinations: Destinations,
    send_apps: SendApps,
}

impl SettingsUi {
    /// 送信先の設定へ移る（送信先がないまま送ろうとしたとき）。
    pub fn show_destinations(&mut self) {
        self.query.clear();
        self.jump = Some(Section::DicomSend);
    }

    /// 送り先のアプリの設定へ移る（「送る ▸」の「アプリを設定...」）。
    pub fn show_send_apps(&mut self) {
        self.query.clear();
        self.jump = Some(Section::SendApps);
    }

    /// 画面を開いたときに呼ぶ。すぐ検索語を打てるようにする。
    pub fn opened(&mut self) {
        self.focus_search = true;
        self.capturing = None;
        self.notice = None;
    }

    /// キー入力待ちなら、このフレームのキー入力を割り当てに使い、他の処理に渡らないよう取り除く。
    /// 割り当てが変わったら true。
    pub fn capture_keys(&mut self, ctx: &egui::Context, keymap: &mut Keymap) -> bool {
        let Some(action) = self.capturing else {
            return false;
        };
        let events = ctx.input_mut(|i| {
            let (keys, rest) = std::mem::take(&mut i.events).into_iter().partition(|e| {
                matches!(
                    e,
                    Event::Key { .. } | Event::Text(_) | Event::Copy | Event::Cut | Event::Paste(_)
                )
            });
            i.events = rest;
            keys
        });
        let (outcome, notice) = capture(action, &events, keymap, Os::CURRENT);
        match outcome {
            Capture::Waiting => {}
            Capture::Cancelled => self.capturing = None,
            Capture::Done => {
                self.capturing = None;
                self.notice = notice.map(|n| (action, n));
                return true;
            }
            Capture::Rejected => {
                self.capturing = None;
                self.notice = notice.map(|n| (action, n));
            }
        }
        false
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        settings: &mut Settings,
        file: Option<&str>,
        file_exists: bool,
    ) -> Response {
        let lang = settings.appearance.language;
        let mut out = Response::default();

        egui::Panel::top("settings_header").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .button(format!("{} {}", icon::ARROW_LEFT, tr(lang, "Back")))
                    .clicked()
                {
                    out.close = true;
                }
                ui.heading(tr(lang, "Settings"));
            });
            ui.horizontal(|ui| {
                ui.label(icon::MAGNIFYING_GLASS);
                let search = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text(tr(lang, "Search settings"))
                        .desired_width((ui.available_width() - 200.0).max(120.0)),
                );
                if std::mem::take(&mut self.focus_search) {
                    search.request_focus();
                }
                let open = ui.add_enabled(
                    file.is_some(),
                    egui::Button::new(format!(
                        "{} {}",
                        icon::FILE_TEXT,
                        tr(lang, "Open settings file")
                    )),
                );
                if open.clicked() {
                    out.open_file = true;
                }
            });
            if let Some(path) = file {
                let note = if file_exists {
                    path.to_owned()
                } else {
                    format!("{path} ({})", tr(lang, "Not saved yet"))
                };
                ui.weak(RichText::new(note).small().monospace());
            }
            ui.add_space(2.0);
        });

        egui::Panel::left("settings_nav")
            .exact_size(170.0)
            .show(ui, |ui| {
                ui.add_space(4.0);
                for s in Section::ALL {
                    if ui
                        .selectable_label(self.current == Some(s), tr(lang, s.title()))
                        .clicked()
                    {
                        self.query.clear();
                        self.jump = Some(s);
                    }
                }
                // キーの割り当ては 90 を超えて長いので、まとまりにも目次から飛べるようにする。
                ui.indent("key_groups", |ui| {
                    for g in KeyGroup::ALL {
                        if g == KeyGroup::Fullscreen {
                            continue;
                        }
                        let viewing =
                            self.current == Some(Section::Keys) && self.current_group == Some(g);
                        if ui.selectable_label(viewing, tr(lang, g.title())).clicked() {
                            self.query.clear();
                            self.key_jump = Some(g);
                        }
                    }
                });
            });

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.set_max_width(760.0);
                    let mut any = false;
                    self.seen = None;
                    self.seen_group = None;
                    for section in Section::ALL {
                        any |= self.section_ui(ui, section, settings, &mut out);
                    }
                    self.current = self.seen;
                    self.current_group = self.seen_group;
                    if !any {
                        ui.add_space(8.0);
                        ui.weak(tr(lang, "No settings match"));
                    }
                });
        });
        out
    }

    /// 1 つの節を描く。検索で 1 項目も残らなければ見出しも出さず false を返す。
    fn section_ui(
        &mut self,
        ui: &mut egui::Ui,
        section: Section,
        settings: &mut Settings,
        out: &mut Response,
    ) -> bool {
        let lang = settings.appearance.language;
        let items: Vec<&Item> = ITEMS
            .iter()
            .filter(|i| i.section == section)
            .filter(|i| {
                matches(
                    &self.query,
                    &[
                        i.key,
                        i.title,
                        i.description,
                        tr(lang, i.title),
                        tr(lang, i.description),
                    ],
                )
            })
            .collect();
        // 全画面スライドショーの間だけ働くキーは、その節に並べる。
        let scope = match section {
            Section::Keys => Some(Scope::Main),
            Section::Fullscreen => Some(Scope::Fullscreen),
            _ => None,
        };
        let actions: Vec<Action> = Action::ALL
            .into_iter()
            .filter(|a| Some(a.scope()) == scope)
            .filter(|a| self.action_matches(*a, &settings.keys, lang))
            .collect();
        let destinations = section == Section::DicomSend
            && matches(
                &self.query,
                &[
                    "dicom_send.destinations",
                    "Destinations",
                    tr(lang, "Destinations"),
                ],
            );
        let send_apps = section == Section::SendApps
            && matches(
                &self.query,
                &["send.apps", "Send to apps", tr(lang, "Send to apps")],
            );
        let log_folder = section == Section::Log
            && matches(
                &self.query,
                &["log.folder", "Log folder", tr(lang, "Log folder")],
            );
        if items.is_empty() && actions.is_empty() && !destinations && !send_apps && !log_folder {
            return false;
        }

        ui.add_space(8.0);
        let heading = ui.label(
            RichText::new(tr(lang, section.title()).to_uppercase())
                .small()
                .strong(),
        );
        if self.jump == Some(section) {
            heading.scroll_to_me(Some(Align::TOP));
            self.jump = None;
        }
        // 見出しが表示の上端より上へ流れた節のうち、最後のものが見ている節。
        // どれも流れていなければ一番上の節。
        if self.seen.is_none() || heading.rect.top() <= ui.clip_rect().top() + VIEWING_MARGIN {
            self.seen = Some(section);
        }
        ui.separator();

        for item in items {
            item_frame(ui, |ui| {
                title_line(ui, tr(lang, item.title), item.key);
                ui.weak(tr(lang, item.description));
                if (item.widget)(settings, ui, lang) {
                    out.changed = true;
                }
            });
        }
        for group in KeyGroup::ALL {
            let in_group: Vec<Action> = actions
                .iter()
                .copied()
                .filter(|a| a.group() == group)
                .collect();
            if in_group.is_empty() {
                continue;
            }
            // 全画面スライドショーの節は 1 つのまとまりだけなので、見出しを重ねない。
            if section == Section::Keys {
                ui.add_space(6.0);
                let heading = ui.label(RichText::new(tr(lang, group.title())).strong());
                if self.key_jump == Some(group) {
                    heading.scroll_to_me(Some(Align::TOP));
                    self.key_jump = None;
                }
                if self.seen_group.is_none()
                    || heading.rect.top() <= ui.clip_rect().top() + VIEWING_MARGIN
                {
                    self.seen_group = Some(group);
                }
            }
            for action in in_group {
                if self.action_ui(ui, action, &mut settings.keys, lang) {
                    out.changed = true;
                }
            }
        }
        if send_apps {
            item_frame(ui, |ui| {
                title_line(ui, tr(lang, "Send to apps"), "send.apps");
                ui.weak(tr(
                    lang,
                    "Apps listed in Send to in the right-click menu. In the arguments, {path} is one file (the app starts once per file), {paths} is every file (the app starts once), {dir} is the folder of the file and {name} is its name. Without any of them, the files are added at the end. Extensions: a regular expression for the whole extension, case-insensitive, e.g. nef|arw|cr[23]; empty means any file.",
                ));
                self.send_apps.ui(ui, settings, out);
            });
        }
        if destinations {
            item_frame(ui, |ui| {
                title_line(ui, tr(lang, "Destinations"), "dicom_send.destinations");
                ui.weak(tr(
                    lang,
                    "Where Send to in the right-click menu sends files. Test sends a C-ECHO to check the settings.",
                ));
                self.destinations.ui(ui, settings, out);
            });
        }
        if log_folder {
            item_frame(ui, |ui| {
                title_line(ui, tr(lang, "Log folder"), "log.folder");
                ui.weak(tr(
                    lang,
                    "The logs folder next to the settings file, where the log files are written.",
                ));
                if ui
                    .button(format!(
                        "{} {}",
                        icon::FOLDER_OPEN,
                        tr(lang, "Open the log folder")
                    ))
                    .clicked()
                {
                    out.open_log_folder = true;
                }
            });
        }
        true
    }

    fn action_matches(&self, action: Action, keymap: &Keymap, lang: Language) -> bool {
        let key = format!("keys.{}", action.id());
        let combos: Vec<String> = keymap
            .keys(action)
            .iter()
            .map(KeyCombo::to_string)
            .collect();
        let group = action.group().title();
        let mut texts = vec![
            key.as_str(),
            action.title(),
            tr(lang, action.title()),
            group,
            tr(lang, group),
        ];
        texts.extend(combos.iter().map(String::as_str));
        matches(&self.query, &texts)
    }

    /// ホットキー 1 行。キーの一覧（クリックで削除）、追加、既定に戻すボタン。
    fn action_ui(
        &mut self,
        ui: &mut egui::Ui,
        action: Action,
        keymap: &mut Keymap,
        lang: Language,
    ) -> bool {
        let mut changed = false;
        let duplicates = keymap.duplicates();
        item_frame(ui, |ui| {
            title_line(
                ui,
                tr(lang, action.title()),
                &format!("keys.{}", action.id()),
            );
            ui.horizontal_wrapped(|ui| {
                let keys = keymap.keys(action).to_vec();
                if keys.is_empty() {
                    ui.weak(tr(lang, "No keys"));
                }
                for k in keys {
                    let duplicated = duplicates
                        .iter()
                        .any(|(c, actions)| *c == k && actions.contains(&action));
                    let reserved = os_conflict(Os::CURRENT, action, k);
                    // 等幅にしない。アイコンフォントは本文（プロポーショナル）にしか組み込んでいないため。
                    let mut text = RichText::new(format!("{k}  {}", icon::X));
                    let mut tip = tr(lang, "Click to remove").to_owned();
                    if duplicated {
                        text = text.color(ui.visuals().error_fg_color);
                        tip = format!("{}\n{tip}", tr(lang, "Also assigned to another action."));
                    } else if let Some(r) = reserved {
                        text = text.color(ui.visuals().warn_fg_color);
                        tip = format!(
                            "{}\n{tip}",
                            tr1(lang, "Used by the OS for \"{}\".", tr(lang, r))
                        );
                    }
                    if ui.button(text).on_hover_text(tip).clicked() {
                        keymap.unassign(action, k);
                        self.notice = None;
                        changed = true;
                    }
                }
                if self.capturing == Some(action) {
                    let waiting = ui.add(
                        egui::Button::new(tr(
                            lang,
                            "Press a key or a mouse side button... (Esc to cancel)",
                        ))
                        .selected(true),
                    );
                    if waiting.clicked() {
                        self.capturing = None;
                    }
                } else if ui
                    .button(icon::PLUS)
                    .on_hover_text(tr(lang, "Add key"))
                    .clicked()
                {
                    self.capturing = Some(action);
                    self.notice = None;
                }
                let reset = ui
                    .add_enabled(
                        !keymap.is_default(action),
                        egui::Button::new(icon::ARROW_COUNTER_CLOCKWISE),
                    )
                    .on_hover_text(tr(lang, "Reset to default"));
                if reset.clicked() {
                    keymap.reset(action);
                    self.notice = None;
                    changed = true;
                }
            });
            if let Some((a, notice)) = self.notice {
                if a == action {
                    let (color, text) = notice_text(ui, notice, lang);
                    ui.colored_label(color, text);
                }
            }
        });
        changed
    }
}

fn notice_text(ui: &egui::Ui, notice: Notice, lang: Language) -> (Color32, String) {
    let v = ui.visuals();
    match notice {
        Notice::Refused(Refusal::UsedBy(other)) => (
            v.error_fg_color,
            tr1(
                lang,
                "Already used by \"{}\". Remove it there first.",
                tr(lang, other.title()),
            ),
        ),
        Notice::Refused(Refusal::AlreadyAssigned) => (
            v.warn_fg_color,
            tr(lang, "Already assigned to this action.").to_owned(),
        ),
        Notice::OsReserved(what) => (
            v.warn_fg_color,
            tr1(
                lang,
                "Assigned. Note: the OS usually uses this key for \"{}\", so it may not reach vw.",
                tr(lang, what),
            ),
        ),
        Notice::Clipboard => (
            v.error_fg_color,
            tr(
                lang,
                "Clipboard keys (Ctrl+C, Ctrl+X, Ctrl+V) are reserved by the OS.",
            )
            .to_owned(),
        ),
    }
}

/// 項目の枠。ホバーで左端に細線を出して、どの項目を触っているかを示す。
fn item_frame(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let r = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        })
        .response;
    if ui.rect_contains_pointer(r.rect) {
        let x = r.rect.left() + 0.5;
        ui.painter().vline(
            x,
            r.rect.y_range(),
            egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
        );
    }
}

/// 節の見出しが表示の上端からこれだけ下にあれば、もうその節を見ているとみなす。
/// 目次で飛んだときに見出しがちょうど上端に来ても、その節が選ばれるように。
const VIEWING_MARGIN: f32 = 24.0;

/// 項目の名前と、設定ファイルでのキー。キーは設定ファイルを直接書く人向けの添え書きなので、
/// 名前より目立たないよう小さく薄くする。
fn title_line(ui: &mut egui::Ui, title: &str, key: &str) {
    ui.horizontal(|ui| {
        ui.strong(title);
        let color = ui.visuals().weak_text_color().gamma_multiply(0.75);
        ui.label(RichText::new(key).size(10.5).monospace().color(color));
    });
}

/// キー入力待ちの結果。
#[derive(Debug, PartialEq, Eq)]
enum Capture {
    /// まだキーが押されていない。
    Waiting,
    Cancelled,
    /// 割り当てた。
    Done,
    /// 割り当てられなかった。
    Rejected,
}

/// キー入力待ちの `action` に、`events` の最初のキーを割り当てる。
/// Esc（修飾キーなし）は取り消し。修飾キー単独の押下は読み飛ばし、続くキーを待つ。
fn capture(
    action: Action,
    events: &[Event],
    keymap: &mut Keymap,
    os: Os,
) -> (Capture, Option<Notice>) {
    for e in events {
        match e {
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if !is_modifier(*key) => {
                let combo = KeyCombo::from_event(*key, *modifiers);
                if combo == KeyCombo::plain(egui::Key::Escape) {
                    return (Capture::Cancelled, None);
                }
                return match keymap.assign(action, combo) {
                    Ok(()) => (
                        Capture::Done,
                        os_conflict(os, action, combo).map(Notice::OsReserved),
                    ),
                    Err(r) => (Capture::Rejected, Some(Notice::Refused(r))),
                };
            }
            Event::PointerButton {
                button,
                pressed: true,
                modifiers,
                ..
            } => {
                let Some(combo) = KeyCombo::from_pointer(*button, *modifiers) else {
                    continue;
                };
                return match keymap.assign(action, combo) {
                    Ok(()) => (Capture::Done, None),
                    Err(r) => (Capture::Rejected, Some(Notice::Refused(r))),
                };
            }
            Event::Copy | Event::Cut | Event::Paste(_) => {
                return (Capture::Rejected, Some(Notice::Clipboard));
            }
            _ => {}
        }
    }
    (Capture::Waiting, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Key, Modifiers};

    fn press(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn empty_query_matches_everything() {
        assert!(matches("", &["Theme"]));
        assert!(matches("   ", &[]));
    }

    #[test]
    fn query_is_case_insensitive() {
        assert!(matches("THEME", &["appearance.theme"]));
    }

    #[test]
    fn every_word_must_be_found_somewhere() {
        let texts = ["Theme", "Follow the OS dark/light setting"];
        assert!(matches("theme dark", &texts));
        assert!(!matches("theme zoom", &texts));
    }

    #[test]
    fn japanese_text_is_searchable() {
        assert!(matches("テーマ", &["Theme", "テーマ"]));
    }

    #[test]
    fn every_setting_has_japanese_title_and_description() {
        for i in &ITEMS {
            assert_ne!(tr(Language::Japanese, i.title), i.title);
            assert_ne!(tr(Language::Japanese, i.description), i.description);
        }
        for s in Section::ALL {
            assert_ne!(tr(Language::Japanese, s.title()), s.title());
        }
    }

    #[test]
    fn setting_keys_are_unique() {
        for (n, i) in ITEMS.iter().enumerate() {
            assert!(!ITEMS[..n].iter().any(|j| j.key == i.key), "{}", i.key);
        }
    }

    #[test]
    fn setting_keys_point_at_real_toml_entries() {
        let toml = Settings::default().to_toml();
        for i in &ITEMS {
            let (table, field) = i.key.rsplit_once('.').unwrap();
            // 表の配列（検索のグループなど）は `[[table.field]]`、入れ子の表（判定の閾値）は
            // `[table.field]` の見出しで書かれる。
            let as_value =
                toml.contains(&format!("[{table}]")) && toml.contains(&format!("{field} = "));
            let as_tables = toml.contains(&format!("[[{}]]", i.key));
            let as_table = toml.contains(&format!("[{}]", i.key));
            assert!(as_value || as_tables || as_table, "{}", i.key);
        }
    }

    #[test]
    fn capture_waits_until_a_key_arrives() {
        let mut map = Keymap::default();
        let (c, n) = capture(
            Action::NewSlot,
            &[Event::Text("x".into())],
            &mut map,
            Os::Linux,
        );
        assert_eq!((c, n), (Capture::Waiting, None));
    }

    #[test]
    fn captured_key_is_assigned() {
        let mut map = Keymap::default();
        let (c, n) = capture(
            Action::NewSlot,
            &[press(Key::Insert, Modifiers::NONE)],
            &mut map,
            Os::Linux,
        );
        assert_eq!((c, n), (Capture::Done, None));
        assert!(map
            .keys(Action::NewSlot)
            .contains(&KeyCombo::plain(Key::Insert)));
    }

    #[test]
    fn modifiers_are_captured_with_the_key() {
        let mut map = Keymap::default();
        let mods = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::NONE
        };
        capture(Action::NewSlot, &[press(Key::O, mods)], &mut map, Os::Linux);
        assert!(map
            .keys(Action::NewSlot)
            .contains(&"Ctrl+Shift+O".parse().unwrap()));
    }

    #[test]
    fn pressing_only_a_modifier_keeps_waiting() {
        let mut map = Keymap::default();
        let events = [
            press(Key::ControlLeft, Modifiers::CTRL),
            press(Key::AltLeft, Modifiers::ALT),
        ];
        let (c, n) = capture(Action::NewSlot, &events, &mut map, Os::Linux);
        assert_eq!((c, n), (Capture::Waiting, None));
    }

    #[test]
    fn modifier_then_key_assigns_the_combination() {
        let mut map = Keymap::default();
        let events = [
            press(Key::AltLeft, Modifiers::ALT),
            press(Key::F4, Modifiers::ALT),
        ];
        let (c, n) = capture(Action::NewSlot, &events, &mut map, Os::Linux);
        assert_eq!(
            (c, n),
            (Capture::Done, Some(Notice::OsReserved("Close window")))
        );
    }

    #[test]
    fn escape_cancels_without_assigning() {
        let mut map = Keymap::default();
        let (c, _) = capture(
            Action::NewSlot,
            &[press(Key::Escape, Modifiers::NONE)],
            &mut map,
            Os::Linux,
        );
        assert_eq!(c, Capture::Cancelled);
        assert_eq!(map, Keymap::default());
    }

    #[test]
    fn escape_with_modifier_can_be_assigned() {
        let mut map = Keymap::default();
        let (c, _) = capture(
            Action::NewSlot,
            &[press(Key::Escape, Modifiers::SHIFT)],
            &mut map,
            Os::Linux,
        );
        assert_eq!(c, Capture::Done);
    }

    #[test]
    fn key_used_elsewhere_is_rejected_with_its_owner() {
        let mut map = Keymap::default();
        let (c, n) = capture(
            Action::NewSlot,
            &[press(Key::ArrowRight, Modifiers::NONE)],
            &mut map,
            Os::Linux,
        );
        assert_eq!(c, Capture::Rejected);
        assert_eq!(n, Some(Notice::Refused(Refusal::UsedBy(Action::NextImage))));
        assert_eq!(map, Keymap::default());
    }

    #[test]
    fn os_key_is_assigned_with_a_warning() {
        let mut map = Keymap::default();
        let (c, n) = capture(
            Action::NewSlot,
            &[press(Key::F4, Modifiers::ALT)],
            &mut map,
            Os::Windows,
        );
        assert_eq!(c, Capture::Done);
        assert_eq!(n, Some(Notice::OsReserved("Close window")));
    }

    #[test]
    fn clipboard_shortcut_cannot_be_assigned() {
        let mut map = Keymap::default();
        let (c, n) = capture(Action::NewSlot, &[Event::Copy], &mut map, Os::Linux);
        assert_eq!((c, n), (Capture::Rejected, Some(Notice::Clipboard)));
    }
}

/// 送信先の表の 1 行。入力の途中（正しくない値）も持てるよう、設定とは別に持つ。
#[derive(Clone)]
struct Row {
    /// 接続確認の結果を、並べ替えや削除で取り違えないための番号。
    id: u64,
    dest: crate::settings::Destination,
    /// ポートの入力欄の文字。数字以外を打った途中も残すため。
    port: String,
}

enum Echo {
    Running(std::sync::mpsc::Receiver<Result<(), String>>),
    Done(Result<(), String>),
}

/// 送信先の表（追加・削除・接続確認）。
#[derive(Default)]
struct Destinations {
    rows: Vec<Row>,
    next_id: u64,
    echoes: std::collections::HashMap<u64, Echo>,
}

impl Destinations {
    /// 正しい行だけを設定に入れたもの。
    fn valid(&self) -> Vec<crate::settings::Destination> {
        self.rows
            .iter()
            .filter(|r| r.dest.problem().is_none())
            .map(|r| r.dest.clone())
            .collect()
    }

    fn row(&mut self, dest: crate::settings::Destination) -> Row {
        self.next_id += 1;
        Row {
            id: self.next_id,
            port: if dest.port == 0 {
                String::new()
            } else {
                dest.port.to_string()
            },
            dest,
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, settings: &mut Settings, out: &mut Response) {
        let lang = settings.appearance.language;
        // 設定ファイルが外で書き換えられたら、表を作り直す。
        if self.valid() != settings.dicom_send.destinations {
            let dests = settings.dicom_send.destinations.clone();
            self.rows = dests.into_iter().map(|d| self.row(d)).collect();
            self.echoes.clear();
        }
        for echo in self.echoes.values_mut() {
            if let Echo::Running(rx) = echo {
                if let Ok(result) = rx.try_recv() {
                    *echo = Echo::Done(result);
                }
            }
        }
        if self.echoes.values().any(|e| matches!(e, Echo::Running(_))) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }

        let mut edited = false;
        let mut remove = None;
        let red = Color32::from_rgb(230, 80, 80);
        egui::Grid::new("destinations")
            .num_columns(6)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                for title in ["Name", "Host", "Port", "AE title"] {
                    ui.weak(tr(lang, title));
                }
                ui.end_row();
                for (i, row) in self.rows.iter_mut().enumerate() {
                    let problem = row.dest.problem();
                    let bad = [
                        row.dest.name.trim().is_empty(),
                        row.dest.host.trim().is_empty(),
                        row.dest.port == 0,
                        !vw_dicom::net::valid_ae_title(&row.dest.ae_title),
                    ];
                    let fields: [(&mut String, f32, bool); 4] = [
                        (&mut row.dest.name, 110.0, bad[0]),
                        (&mut row.dest.host, 140.0, bad[1]),
                        (&mut row.port, 56.0, bad[2]),
                        (&mut row.dest.ae_title, 130.0, bad[3]),
                    ];
                    for (text, width, bad) in fields {
                        // グリッドのセルでは前の描画の列幅までしか広がらないため、幅は下限で与える。
                        let r = ui.add(
                            egui::TextEdit::singleline(text)
                                .desired_width(width)
                                .min_size(egui::vec2(width, 0.0)),
                        );
                        if bad {
                            ui.painter().rect_stroke(
                                r.rect,
                                2.0,
                                egui::Stroke::new(1.0, red),
                                egui::StrokeKind::Outside,
                            );
                        }
                        edited |= r.changed();
                    }
                    row.dest.port = row.port.trim().parse().unwrap_or(0);
                    ui.horizontal(|ui| {
                        let test = ui.add_enabled(
                            problem.is_none()
                                && !matches!(self.echoes.get(&row.id), Some(Echo::Running(_))),
                            egui::Button::new(tr(lang, "Test")),
                        );
                        if test.clicked() {
                            let endpoint = vw_dicom::net::Endpoint {
                                calling_ae: settings.dicom_send.ae_title.clone(),
                                host: row.dest.host.trim().to_owned(),
                                port: row.dest.port,
                                called_ae: row.dest.ae_title.clone(),
                                timeout: settings.dicom_send.timeout(),
                            };
                            let rx = crate::background::compute(ui.ctx(), move || {
                                vw_dicom::net::echo(&endpoint)
                            });
                            self.echoes.insert(row.id, Echo::Running(rx));
                        }
                        if ui
                            .button(icon::X)
                            .on_hover_text(tr(lang, "Remove destination"))
                            .clicked()
                        {
                            remove = Some(i);
                        }
                        match self.echoes.get(&row.id) {
                            Some(Echo::Running(_)) => {
                                ui.spinner();
                            }
                            Some(Echo::Done(Ok(()))) => {
                                ui.colored_label(
                                    Color32::from_rgb(90, 180, 90),
                                    tr(lang, "Connected"),
                                );
                            }
                            Some(Echo::Done(Err(e))) => {
                                ui.colored_label(red, tr(lang, "Failed"))
                                    .on_hover_text(e.as_str());
                            }
                            None => {
                                if let Some(p) = problem {
                                    ui.colored_label(red, tr_text(lang, p));
                                }
                            }
                        }
                    });
                    ui.end_row();
                }
            });
        // 終わった接続確認は 1 度だけログに出す。
        for row in &self.rows {
            if let Some(Echo::Done(result)) = self.echoes.get(&row.id) {
                let key = ("echo_logged", row.id);
                let logged = ui
                    .data(|d| d.get_temp::<bool>(egui::Id::new(key)))
                    .unwrap_or(false);
                if !logged {
                    ui.data_mut(|d| d.insert_temp(egui::Id::new(key), true));
                    let target = format!("{} ({})", row.dest.name, row.dest.address());
                    out.logs.push(match result {
                        Ok(()) => (true, format!("C-ECHO {target}: OK")),
                        Err(e) => (false, format!("C-ECHO {target}: {e}")),
                    });
                }
            }
        }
        if ui
            .button(format!("{} {}", icon::PLUS, tr(lang, "Add destination")))
            .clicked()
        {
            let row = self.row(crate::settings::Destination {
                port: 104,
                ..Default::default()
            });
            self.rows.push(row);
            edited = true;
        }
        if let Some(i) = remove {
            let row = self.rows.remove(i);
            self.echoes.remove(&row.id);
            edited = true;
        }
        if edited {
            // 行を書き換えたら、前の接続確認の結果はもう当てにならない。
            for row in &self.rows {
                if matches!(self.echoes.get(&row.id), Some(Echo::Done(_))) {
                    let same = settings.dicom_send.destinations.contains(&row.dest);
                    if !same {
                        self.echoes.remove(&row.id);
                    }
                }
            }
            let valid = self.valid();
            if valid != settings.dicom_send.destinations {
                settings.dicom_send.destinations = valid;
                out.changed = true;
            }
        }
    }
}

/// 送り先のアプリの表（追加・削除・並べ替え・関連付けからの取り込み）。
#[derive(Default)]
struct SendApps {
    rows: Vec<crate::settings::SendApp>,
    /// 関連付けを調べている。
    importing: Option<std::sync::mpsc::Receiver<Vec<crate::settings::SendApp>>>,
}

impl SendApps {
    fn valid(&self) -> Vec<crate::settings::SendApp> {
        self.rows
            .iter()
            .filter(|r| r.problem().is_none())
            .cloned()
            .collect()
    }

    fn ui(&mut self, ui: &mut egui::Ui, settings: &mut Settings, out: &mut Response) {
        use crate::settings::Accepts;
        let lang = settings.appearance.language;
        let saved = settings.send.apps.clone().unwrap_or_default();
        // 設定ファイルが外で書き換えられたら、表を作り直す。
        if self.valid() != saved {
            self.rows = saved;
        }
        let mut edited = false;
        let mut remove = None;
        let mut swap = None;
        let red = Color32::from_rgb(230, 80, 80);
        let count = self.rows.len();
        egui::Grid::new("send_apps")
            .num_columns(6)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                for title in ["Name", "Program", "Arguments", "Accepts", "Extensions"] {
                    ui.weak(tr(lang, title));
                }
                ui.end_row();
                for (i, row) in self.rows.iter_mut().enumerate() {
                    let problem = row.problem();
                    let bad = [
                        row.name.trim().is_empty(),
                        row.program.trim().is_empty(),
                        vw_launch::args::split(&row.args).is_err(),
                    ];
                    let fields: [(&mut String, f32, bool); 3] = [
                        (&mut row.name, 120.0, bad[0]),
                        (&mut row.program, 240.0, bad[1]),
                        (&mut row.args, 140.0, bad[2]),
                    ];
                    for (text, width, bad) in fields {
                        // グリッドのセルでは前の描画の列幅までしか広がらないため、幅は下限で与える。
                        let r = ui.add(
                            egui::TextEdit::singleline(text)
                                .desired_width(width)
                                .min_size(egui::vec2(width, 0.0)),
                        );
                        if bad {
                            ui.painter().rect_stroke(
                                r.rect,
                                2.0,
                                egui::Stroke::new(1.0, red),
                                egui::StrokeKind::Outside,
                            );
                        }
                        edited |= r.changed();
                    }
                    egui::ComboBox::from_id_salt(("send_app_accepts", i))
                        .selected_text(tr(lang, row.accepts.title()))
                        .show_ui(ui, |ui| {
                            for a in Accepts::ALL {
                                edited |= ui
                                    .selectable_value(&mut row.accepts, a, tr(lang, a.title()))
                                    .changed();
                            }
                        });
                    ui.horizontal(|ui| {
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut row.extensions)
                                .desired_width(110.0)
                                .min_size(egui::vec2(110.0, 0.0))
                                .hint_text(tr(lang, "any")),
                        );
                        if problem == Some("extensions is not a valid regular expression") {
                            ui.painter().rect_stroke(
                                r.rect,
                                2.0,
                                egui::Stroke::new(1.0, red),
                                egui::StrokeKind::Outside,
                            );
                        }
                        edited |= r.changed();
                        if ui
                            .add_enabled(i > 0, egui::Button::new(icon::ARROW_UP))
                            .clicked()
                        {
                            swap = Some((i - 1, i));
                        }
                        if ui
                            .add_enabled(i + 1 < count, egui::Button::new(icon::ARROW_DOWN))
                            .clicked()
                        {
                            swap = Some((i, i + 1));
                        }
                        if ui
                            .button(icon::X)
                            .on_hover_text(tr(lang, "Remove app"))
                            .clicked()
                        {
                            remove = Some(i);
                        }
                        if let Some(p) = problem {
                            ui.colored_label(red, tr_text(lang, p));
                        }
                    });
                    ui.end_row();
                }
            });
        ui.horizontal(|ui| {
            if ui
                .button(format!("{} {}", icon::PLUS, tr(lang, "Add app")))
                .clicked()
            {
                self.rows.push(crate::settings::SendApp {
                    args: "{paths}".into(),
                    ..Default::default()
                });
                edited = true;
            }
            let importing = self.importing.is_some();
            if ui
                .add_enabled(
                    !importing,
                    egui::Button::new(tr(lang, "Add from file associations")),
                )
                .on_hover_text(tr(
                    lang,
                    "Adds the apps the OS opens JPEG and camera RAW files with.",
                ))
                .clicked()
            {
                self.importing = Some(crate::background::compute(ui.ctx(), || {
                    vw_launch::assoc::associated_apps(&crate::send_app::ASSOCIATED_EXTENSIONS)
                        .into_iter()
                        .map(crate::send_app::from_associated)
                        .collect()
                }));
            }
            if importing {
                ui.spinner();
            }
        });
        if let Some(found) = self.importing.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.importing = None;
            let added = crate::send_app::merge(&mut self.rows, found);
            out.logs.push((
                true,
                format!("Send to apps: added {added} app(s) from file associations"),
            ));
            edited = true;
        }
        if let Some((a, b)) = swap {
            self.rows.swap(a, b);
            edited = true;
        }
        if let Some(i) = remove {
            self.rows.remove(i);
            edited = true;
        }
        if edited {
            let valid = self.valid();
            if settings.send.apps.as_ref() != Some(&valid) {
                settings.send.apps = Some(valid);
                out.changed = true;
            }
        }
    }
}
