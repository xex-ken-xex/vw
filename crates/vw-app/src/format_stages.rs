//! DICOM 以外の形式の、ファイルの段。デコーダが残したメタデータから「格納値 → … → 表示」の
//! 段を組み立てる（値は変えられない。色導出のウインドウに見せるだけ）。
//!
//! デコーダに段の記録の口を足さずメタデータから組むのは、段に出したい値（色空間、ビット深度、
//! ガンマ、ICC、向き、現像の値）がメタデータ欄にすでに出ていて、同じ値を 2 か所で持つと
//! 食い違う元になるため。メタデータにない値は段に出さない。

use crate::loader::ImageInfo;

/// 1 つの段。`title` と `note` は英語の文言（表示時に訳す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    pub title: &'static str,
    /// 「名前: 値」の行。
    pub lines: Vec<(String, String)>,
    pub note: Option<&'static str>,
}

impl Stage {
    fn new(title: &'static str) -> Self {
        Self {
            title,
            lines: Vec::new(),
            note: None,
        }
    }

    fn line(mut self, name: &str, value: &str) -> Self {
        self.lines.push((name.to_owned(), value.to_owned()));
        self
    }

    fn note(mut self, note: &'static str) -> Self {
        self.note = Some(note);
        self
    }
}

/// `info` の形式の段。先頭は格納値、末尾の「表示」は含まない。
pub fn stages(info: &ImageInfo) -> Vec<Stage> {
    let m = &info.metadata;
    let get = |key: &str| m.get(&info.plugin, key);
    let mut stored = Stage::new("Stored values").line("Format", &info.plugin);
    let mut out = Vec::new();
    match info.plugin.as_str() {
        "JPEG" => {
            for key in ["Components", "Coding"] {
                if let Some(v) = get(key) {
                    stored = stored.line(key, v);
                }
            }
            out.push(stored);
            let space = get("Color space").unwrap_or("YCbCr");
            let mut conversion = Stage::new("Color conversion").line("Stored color space", space);
            conversion = if space.contains("CMYK") || space.contains("YCCK") {
                conversion.note("CMYK is inverted (Adobe) and converted to RGB.")
            } else if space.contains("Luma") {
                conversion.note("Grayscale, no conversion.")
            } else if space.contains("RGB") {
                conversion.note("Already RGB, no conversion.")
            } else {
                conversion.note("YCbCr to RGB (JFIF).")
            };
            out.push(conversion);
            out.extend(icc(get("ICC profile")));
            out.extend(orientation(m.get("EXIF", "Orientation")));
        }
        "PNG" => {
            for key in ["Color type", "Bit depth", "Interlaced"] {
                if let Some(v) = get(key) {
                    stored = stored.line(key, v);
                }
            }
            out.push(stored);
            let color = get("Color type").unwrap_or("");
            let depth = get("Bit depth").unwrap_or("8");
            let mut samples = Stage::new("Samples");
            samples = if color.contains("Indexed") {
                samples
                    .line("Palette", "PLTE")
                    .note("Each index is looked up in the palette (tRNS gives alpha).")
            } else if depth == "16" {
                samples
                    .line("Bit depth", "16")
                    .note("16-bit samples are kept; the display maps the full range to 8 bits.")
            } else if depth == "8" {
                samples.line("Bit depth", "8").note("Used as is.")
            } else {
                samples
                    .line("Bit depth", depth)
                    .note("Samples below 8 bits are scaled up to 8 bits.")
            };
            out.push(samples);
            let mut color_stage = Stage::new("Color management");
            let mut any = false;
            for (key, name) in [("Gamma", "gAMA"), ("sRGB intent", "sRGB")] {
                if let Some(v) = get(key) {
                    color_stage = color_stage.line(name, v);
                    any = true;
                }
            }
            if get("ICC profile").is_some() {
                color_stage = color_stage.line("iCCP", "present");
                any = true;
            }
            if any {
                out.push(color_stage.note("Present in the file but not applied by vw."));
            }
        }
        "TIFF" => {
            for key in [
                "Compression",
                "Photometric",
                "Bits per sample",
                "Sample format",
                "Tiles",
            ] {
                if let Some(v) = get(key) {
                    stored = stored.line(key, v);
                }
            }
            out.push(stored);
            if let Some(p) = get("Photometric") {
                let stage = Stage::new("Color conversion").line("Photometric", p);
                out.push(
                    if p.contains("white is zero") || p.contains("WhiteIsZero") {
                        stage.note("Inverted so that zero is black.")
                    } else if p.contains("alette") {
                        stage.note("Each index is looked up in the color map.")
                    } else if p.contains("YCbCr") {
                        stage.note("YCbCr to RGB.")
                    } else if p.contains("CMYK") || p.contains("eparated") {
                        stage.note("CMYK to RGB.")
                    } else {
                        stage.note("Used as is.")
                    },
                );
            }
            out.extend(orientation(m.get("EXIF", "Orientation")));
        }
        "WebP" => {
            if let Some(v) = get("Coding") {
                stored = stored.line("Coding", v);
            }
            out.push(stored);
            out.extend(icc(get("ICC profile")));
            out.extend(orientation(m.get("EXIF", "Orientation")));
        }
        "GIF" => {
            for key in ["Global colors", "Transparency"] {
                if let Some(v) = get(key) {
                    stored = stored.line(key, v);
                }
            }
            out.push(stored);
            out.push(
                Stage::new("Palette")
                    .note("Each index is looked up in the global or local color table."),
            );
        }
        "BMP" => {
            for key in ["Header", "Bits per pixel", "Compression", "Palette colors"] {
                if let Some(v) = get(key) {
                    stored = stored.line(key, v);
                }
            }
            out.push(stored);
            if get("Palette colors").is_some() {
                out.push(Stage::new("Palette").note("Each index is looked up in the color table."));
            }
        }
        "RAW" => {
            for key in ["Make", "Model"] {
                if let Some(v) = get(key) {
                    stored = stored.line(key, v);
                }
            }
            let developed = get("Shown").is_some_and(|s| s.starts_with("Developed"));
            out.push(stored);
            if developed {
                let mut levels = Stage::new("Levels");
                for key in ["Black level", "White level"] {
                    if let Some(v) = get(key) {
                        levels = levels.line(key, v);
                    }
                }
                out.push(levels.note("Black is subtracted and white scaled to 1."));
                let mut wb = Stage::new("White balance");
                if let Some(v) = get("White balance") {
                    wb = wb.line("Coefficients", v);
                }
                out.push(wb.note("Camera white balance from the file."));
                out.push(
                    Stage::new("Color matrix")
                        .note("Camera to sRGB matrix from rawler's camera data."),
                );
                let mut gain = Stage::new("Brightness");
                if let Some(v) = get("Brightness gain") {
                    gain = gain.line("Gain", v);
                }
                out.push(gain.note("Matched to the median brightness of the embedded preview."));
                out.push(Stage::new("Gamma").note("sRGB curve applied after the gain."));
            } else {
                out.push(
                    Stage::new("Embedded preview")
                        .note("The camera's own JPEG is shown; press D to develop."),
                );
            }
            out.extend(orientation(get("Orientation")));
        }
        _ => out.push(stored),
    }
    out
}

fn icc(present: Option<&str>) -> Option<Stage> {
    present.map(|_| {
        Stage::new("ICC profile")
            .line("ICC profile", "present")
            .note("Present in the file but not applied by vw.")
    })
}

/// 向きの段。EXIF の向きは kamadak-exif の文（"row 0 at top and column 0 at left" が 1）で、
/// RAW の向きは番号で入るので、どちらも番号に戻してから、正位置（1）以外を段にする。
fn orientation(value: Option<&str>) -> Option<Stage> {
    const EXIF_TEXT: [&str; 8] = [
        "row 0 at top and column 0 at left",
        "row 0 at top and column 0 at right",
        "row 0 at bottom and column 0 at right",
        "row 0 at bottom and column 0 at left",
        "row 0 at left and column 0 at top",
        "row 0 at right and column 0 at top",
        "row 0 at right and column 0 at bottom",
        "row 0 at left and column 0 at bottom",
    ];
    const NOTES: [&str; 8] = [
        "Upright.",
        "Mirrored horizontally.",
        "Rotated 180°.",
        "Mirrored vertically.",
        "Mirrored and rotated 90° clockwise.",
        "Rotated 90° clockwise.",
        "Mirrored and rotated 90° counter-clockwise.",
        "Rotated 90° counter-clockwise.",
    ];
    let v = value?;
    let n = EXIF_TEXT
        .iter()
        .position(|t| *t == v)
        .map(|i| i + 1)
        .or_else(|| v.parse::<usize>().ok())?;
    if n <= 1 || n > 8 {
        return None;
    }
    Some(
        Stage::new("Orientation")
            .line("Orientation", &n.to_string())
            .note(NOTES[n - 1]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use vw_core::Metadata;

    fn info(plugin: &str, entries: &[(&str, &str, &str)]) -> ImageInfo {
        let mut metadata = Metadata::default();
        for (g, k, v) in entries {
            metadata.push(*g, *k, v);
        }
        ImageInfo {
            width: 4,
            height: 4,
            color: vw_core::ColorModel::Rgb,
            bits_per_sample: 8,
            significant_bits: 8,
            metadata,
            develop_size: None,
            plugin: plugin.into(),
            frames: 1,
            file_size: 0,
            read: Default::default(),
            decode: Default::default(),
            convert: Default::default(),
        }
    }

    fn titles(stages: &[Stage]) -> Vec<&'static str> {
        stages.iter().map(|s| s.title).collect()
    }

    #[test]
    fn jpeg_shows_color_conversion_and_only_the_tags_it_has() {
        let plain = info(
            "JPEG",
            &[
                ("JPEG", "Color space", "YCbCr"),
                ("JPEG", "Components", "3"),
            ],
        );
        assert_eq!(
            titles(&stages(&plain)),
            ["Stored values", "Color conversion"]
        );
        let full = info(
            "JPEG",
            &[
                ("JPEG", "Color space", "CMYK"),
                ("JPEG", "ICC profile", "present"),
                ("EXIF", "Orientation", "row 0 at right and column 0 at top"),
            ],
        );
        let s = stages(&full);
        assert_eq!(
            titles(&s),
            [
                "Stored values",
                "Color conversion",
                "ICC profile",
                "Orientation"
            ]
        );
        assert!(s[1].note.unwrap().contains("CMYK"));
        assert_eq!(s[3].lines, [("Orientation".to_owned(), "6".to_owned())]);
    }

    #[test]
    fn upright_orientation_is_not_a_stage() {
        let upright = info(
            "JPEG",
            &[("EXIF", "Orientation", "row 0 at top and column 0 at left")],
        );
        assert_eq!(
            titles(&stages(&upright)),
            ["Stored values", "Color conversion"]
        );
        let raw_upright = info("RAW", &[("RAW", "Orientation", "1")]);
        assert_eq!(
            titles(&stages(&raw_upright)),
            ["Stored values", "Embedded preview"]
        );
        let raw_rotated = info("RAW", &[("RAW", "Orientation", "8")]);
        let s = stages(&raw_rotated);
        assert_eq!(
            s.last().unwrap().lines,
            [("Orientation".to_owned(), "8".to_owned())]
        );
        assert!(s
            .last()
            .unwrap()
            .note
            .unwrap()
            .contains("counter-clockwise"));
    }

    #[test]
    fn png_tells_how_samples_and_color_chunks_are_treated() {
        let sixteen = info(
            "PNG",
            &[
                ("PNG", "Color type", "Grayscale"),
                ("PNG", "Bit depth", "16"),
                ("PNG", "Gamma", "0.45455"),
            ],
        );
        let s = stages(&sixteen);
        assert_eq!(titles(&s), ["Stored values", "Samples", "Color management"]);
        assert!(s[1].note.unwrap().contains("16-bit"));
        assert_eq!(s[2].lines, [("gAMA".to_owned(), "0.45455".to_owned())]);
        let indexed = info(
            "PNG",
            &[("PNG", "Color type", "Indexed"), ("PNG", "Bit depth", "8")],
        );
        let s = stages(&indexed);
        assert_eq!(titles(&s), ["Stored values", "Samples"]);
        assert!(s[1].note.unwrap().contains("palette"));
    }

    #[test]
    fn raw_lists_the_develop_steps_only_when_developed() {
        let preview = info(
            "RAW",
            &[("RAW", "Shown", "Embedded preview (JPEG made by the camera)")],
        );
        assert_eq!(
            titles(&stages(&preview)),
            ["Stored values", "Embedded preview"]
        );
        let developed = info(
            "RAW",
            &[
                ("RAW", "Shown", "Developed from sensor data"),
                ("RAW", "Black level", "512"),
                ("RAW", "White level", "16383"),
                ("RAW", "White balance", "2.1, 1.0, 1.5"),
                ("RAW", "Brightness gain", "1.80"),
            ],
        );
        let s = stages(&developed);
        assert_eq!(
            titles(&s),
            [
                "Stored values",
                "Levels",
                "White balance",
                "Color matrix",
                "Brightness",
                "Gamma"
            ]
        );
        assert_eq!(s[4].lines, [("Gain".to_owned(), "1.80".to_owned())]);
    }

    #[test]
    fn unknown_formats_still_get_the_stored_stage() {
        assert_eq!(titles(&stages(&info("HEIC", &[]))), ["Stored values"]);
    }
}
