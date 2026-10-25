use std::path::{Path, PathBuf};

use vw_core::{DecoderPlugin, PixelData, VwImage};

use super::frame::{is_calibration_folder, seconds};
use super::*;
use crate::{FitsPlugin, SharedOptions};

const W: usize = 4;
const H: usize = 2;

/// テストごとの空のフォルダ。
fn temp_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("vw-fits-calibration-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 実数の W x H の FITS を書く。`cards` の値は FITS に書く形のまま（文字列は引用符ごと）。
fn write(path: &Path, cards: &[(&str, &str)], values: &[f32]) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut header = String::new();
    let mut push =
        |k: &str, v: &str| header.push_str(&format!("{:<80}", format!("{k:<8}= {v:>20}")));
    push("SIMPLE", "T");
    push("BITPIX", "-32");
    push("NAXIS", "2");
    push("NAXIS1", &W.to_string());
    push("NAXIS2", &H.to_string());
    for (k, v) in cards {
        push(k, v);
    }
    header.push_str(&format!("{:<80}", "END"));
    let mut out = header.into_bytes();
    out.resize(out.len().next_multiple_of(2880), b' ');
    out.extend(values.iter().flat_map(|v| v.to_be_bytes()));
    out.resize(out.len().next_multiple_of(2880), 0);
    std::fs::write(path, out).unwrap();
    path.to_path_buf()
}

fn filled(v: f32) -> Vec<f32> {
    vec![v; W * H]
}

fn plugin(options: Options) -> (FitsPlugin, Calibration) {
    let calibration = Calibration::default();
    calibration.set_options(options);
    (
        FitsPlugin::new(SharedOptions::default(), calibration.clone()),
        calibration,
    )
}

fn open(p: &FitsPlugin, path: &Path) -> VwImage {
    p.decode_frame_at(path, &std::fs::read(path).unwrap(), 0)
        .unwrap()
}

fn values(img: &VwImage) -> Vec<f32> {
    match &img.data {
        PixelData::F32(v) => v.clone(),
        PixelData::U16(v) => v.iter().map(|&x| x as f32).collect(),
        PixelData::U8(v) => v.iter().map(|&x| x as f32).collect(),
    }
}

fn state(img: &VwImage) -> Option<&str> {
    img.metadata.get(GROUP, STATE)
}

const LIGHT: &[(&str, &str)] = &[
    ("IMAGETYP", "'LIGHT'"),
    ("EXPTIME", "300.0"),
    ("CCD-TEMP", "-10.0"),
    ("GAIN", "100"),
    ("FILTER", "'Ha'"),
    ("DATE-OBS", "'2026-10-24T22:00:00'"),
];

fn frame_of(path: &str, cards: &[(&str, &str)]) -> Frame {
    let mut all: Vec<(String, String)> =
        vec![("NAXIS1".into(), "4".into()), ("NAXIS2".into(), "2".into())];
    all.extend(
        cards
            .iter()
            .map(|(k, v)| (k.to_string(), v.trim_matches('\'').to_string())),
    );
    Frame::read(Path::new(path), &|key| {
        all.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    })
    .unwrap()
}

#[test]
fn image_types_of_each_capture_program_are_recognised() {
    for (value, kind) in [
        ("LIGHT", Kind::Light),
        ("Light Frame", Kind::Light),
        ("object", Kind::Light),
        ("DARK", Kind::Dark),
        ("Dark Frame", Kind::Dark),
        ("Master Dark", Kind::Dark),
        ("FLAT", Kind::Flat),
        ("Flat Field", Kind::Flat),
        ("BIAS", Kind::Bias),
        ("Bias Frame", Kind::Bias),
        ("zero", Kind::Bias),
        ("DARKFLAT", Kind::DarkFlat),
        ("Flat Dark", Kind::DarkFlat),
    ] {
        assert_eq!(
            frame_of("/x/a.fits", &[("IMAGETYP", value)]).kind,
            kind,
            "{value}"
        );
    }
}

#[test]
fn without_an_image_type_the_folder_name_tells_the_kind() {
    assert_eq!(frame_of("/n/darks/a.fits", &[]).kind, Kind::Dark);
    assert_eq!(frame_of("/n/FLAT/Ha/a.fits", &[]).kind, Kind::Flat);
    assert_eq!(frame_of("/n/biases/a.fits", &[]).kind, Kind::Bias);
    assert_eq!(frame_of("/n/dark_flats/a.fits", &[]).kind, Kind::DarkFlat);
    assert_eq!(frame_of("/n/M31/a.fits", &[]).kind, Kind::Light);
}

#[test]
fn masters_are_told_from_single_frames() {
    assert!(frame_of("/n/a.fits", &[("IMAGETYP", "Master Dark")]).master);
    assert!(frame_of("/n/master_dark_300s.fits", &[("IMAGETYP", "DARK")]).master);
    assert!(frame_of("/n/masters/d.fits", &[("IMAGETYP", "DARK")]).master);
    assert!(frame_of("/n/d.fits", &[("IMAGETYP", "DARK"), ("NCOMBINE", "20")]).master);
    assert!(!frame_of("/n/darks/dark_0001.fits", &[("IMAGETYP", "DARK")]).master);
}

#[test]
fn calibration_folders_are_known_by_name() {
    for name in [
        "darks",
        "DARK",
        "Flats",
        "biases",
        "DarkFlats",
        "calibration",
        "Masters",
    ] {
        assert!(is_calibration_folder(name), "{name}");
    }
    for name in ["lights", "M31", "2026-10-24"] {
        assert!(!is_calibration_folder(name), "{name}");
    }
}

#[test]
fn a_night_runs_from_noon_to_noon() {
    let evening = frame_of("/a.fits", &[("DATE-OBS", "2026-10-24T22:00:00")]);
    let morning = frame_of("/b.fits", &[("DATE-OBS", "2026-10-25T03:30:00.5")]);
    let next = frame_of("/c.fits", &[("DATE-OBS", "2026-10-25T21:00:00")]);
    assert_eq!(evening.night(), morning.night());
    assert_ne!(morning.night(), next.night());
    assert_eq!(seconds("1970-01-02"), Some(86400.0));
}

#[test]
fn dark_is_subtracted_and_flat_divides_after_its_dark_flat() {
    let dir = temp_dir("full");
    let light = write(&dir.join("lights/m31_001.fits"), LIGHT, &filled(1100.0));
    write(
        &dir.join("darks/master_dark_300s.fits"),
        &[
            ("IMAGETYP", "'Master Dark'"),
            ("EXPTIME", "300.0"),
            ("CCD-TEMP", "-10.5"),
            ("GAIN", "100"),
        ],
        &filled(100.0),
    );
    let mut flat = filled(1100.0);
    flat[0] = 600.0; // 周辺減光で半分の明るさ
    write(
        &dir.join("flats/master_flat_Ha.fits"),
        &[
            ("IMAGETYP", "'Master Flat'"),
            ("EXPTIME", "2.0"),
            ("FILTER", "'Ha'"),
        ],
        &flat,
    );
    write(
        &dir.join("darkflats/master_darkflat_2s.fits"),
        &[("IMAGETYP", "'Master Dark Flat'"), ("EXPTIME", "2.0")],
        &filled(100.0),
    );
    let (p, _) = plugin(Options::default());
    let img = open(&p, &light);
    assert_eq!(state(&img), Some(APPLIED));
    let v = values(&img);
    // フラットは 1000（中央値）で割って 1、角は 0.5。
    assert!((v[1] - 1000.0).abs() < 1e-3, "{v:?}");
    assert!((v[0] - 2000.0).abs() < 1e-3, "{v:?}");
    let used = img.metadata.get(GROUP, "Flat dark").unwrap();
    assert!(used.contains("master_darkflat_2s"), "{used}");
}

#[test]
fn darks_are_matched_by_exposure_temperature_and_gain() {
    let dir = temp_dir("dark-match");
    let light = write(&dir.join("lights/l.fits"), LIGHT, &filled(500.0));
    let dark = |name: &str, exp: &str, temp: &str, gain: &str, v: f32| {
        write(
            &dir.join("darks").join(name),
            &[
                ("IMAGETYP", "'Master Dark'"),
                ("EXPTIME", exp),
                ("CCD-TEMP", temp),
                ("GAIN", gain),
            ],
            &filled(v),
        );
    };
    dark("a.fits", "120.0", "-10.0", "100", 1.0);
    dark("b.fits", "300.0", "-20.0", "100", 2.0);
    dark("c.fits", "300.0", "-10.0", "200", 3.0);
    dark("d.fits", "300.2", "-11.0", "100", 4.0);
    let (p, _) = plugin(Options {
        use_flat: false,
        ..Options::default()
    });
    let img = open(&p, &light);
    assert_eq!(values(&img)[0], 496.0);
    assert!(img.metadata.get(GROUP, "Dark").unwrap().contains("d.fits"));
}

#[test]
fn nothing_is_applied_when_no_dark_matches_and_the_reason_is_shown() {
    let dir = temp_dir("dark-none");
    let light = write(&dir.join("lights/l.fits"), LIGHT, &filled(500.0));
    write(
        &dir.join("darks/master.fits"),
        &[
            ("IMAGETYP", "'Master Dark'"),
            ("EXPTIME", "300.0"),
            ("CCD-TEMP", "-5.0"),
        ],
        &filled(100.0),
    );
    let (p, _) = plugin(Options {
        use_flat: false,
        ..Options::default()
    });
    let img = open(&p, &light);
    assert_eq!(state(&img), Some(NOT_APPLIED));
    assert_eq!(values(&img)[0], 500.0);
    let why = img.metadata.get(GROUP, "Dark").unwrap();
    assert!(why.contains("°C"), "{why}");
}

#[test]
fn single_frames_without_a_master_are_not_used() {
    let dir = temp_dir("singles");
    let light = write(&dir.join("lights/l.fits"), LIGHT, &filled(500.0));
    for i in 0..3 {
        write(
            &dir.join(format!("darks/dark_{i:04}.fits")),
            &[("IMAGETYP", "'DARK'"), ("EXPTIME", "300.0")],
            &filled(100.0),
        );
    }
    let (p, _) = plugin(Options::default());
    let img = open(&p, &light);
    assert_eq!(state(&img), Some(NOT_APPLIED));
    assert!(img
        .metadata
        .get(GROUP, "Dark")
        .unwrap()
        .contains("no master"));
}

#[test]
fn flats_must_have_the_light_s_filter() {
    let dir = temp_dir("flat-filter");
    let light = write(&dir.join("lights/l.fits"), LIGHT, &filled(500.0));
    write(
        &dir.join("flats/master_flat_OIII.fits"),
        &[("IMAGETYP", "'Master Flat'"), ("FILTER", "'OIII'")],
        &filled(2.0),
    );
    let (p, _) = plugin(Options {
        use_dark: false,
        use_bias: false,
        ..Options::default()
    });
    let img = open(&p, &light);
    assert_eq!(state(&img), Some(NOT_APPLIED));
    assert!(img.metadata.get(GROUP, "Flat").unwrap().contains("Ha"));
}

#[test]
fn bias_is_taken_from_the_flat_when_there_is_no_dark_flat() {
    let dir = temp_dir("flat-bias");
    let light = write(&dir.join("LIGHT/l.fits"), LIGHT, &filled(500.0));
    let mut flat = filled(300.0);
    flat[0] = 200.0;
    write(
        &dir.join("FLAT/master_flat.fits"),
        &[
            ("IMAGETYP", "'Master Flat'"),
            ("FILTER", "'Ha'"),
            ("EXPTIME", "1.0"),
        ],
        &flat,
    );
    write(
        &dir.join("BIAS/master_bias.fits"),
        &[("IMAGETYP", "'Master Bias'"), ("GAIN", "100")],
        &filled(100.0),
    );
    let (p, _) = plugin(Options {
        use_dark: false,
        ..Options::default()
    });
    let img = open(&p, &light);
    let v = values(&img);
    // ライトからもバイアスを引き、フラットは (300 − 100) で 1、角は 0.5。
    assert!((v[1] - 400.0).abs() < 1e-3, "{v:?}");
    assert!((v[0] - 800.0).abs() < 1e-3, "{v:?}");
}

#[test]
fn darks_of_another_exposure_are_scaled_only_when_asked() {
    let dir = temp_dir("scaling");
    let light = write(&dir.join("lights/l.fits"), LIGHT, &filled(1000.0));
    write(
        &dir.join("darks/master_dark_150s.fits"),
        &[("IMAGETYP", "'Master Dark'"), ("EXPTIME", "150.0")],
        &filled(150.0),
    );
    write(
        &dir.join("bias/master_bias.fits"),
        &[("IMAGETYP", "'Master Bias'")],
        &filled(100.0),
    );
    let o = Options {
        use_flat: false,
        ..Options::default()
    };
    let (p, _) = plugin(o.clone());
    // 伸ばさないなら、露出が合わないダークは使わずバイアスだけ引く。
    assert_eq!(values(&open(&p, &light))[0], 900.0);
    let (p, _) = plugin(Options {
        dark_scaling: DarkScaling::Exposure,
        ..o
    });
    // 1000 − (100 + 2 × (150 − 100))
    assert_eq!(values(&open(&p, &light))[0], 800.0);
}

#[test]
fn a_light_can_be_seen_without_calibration_and_back() {
    let dir = temp_dir("toggle");
    let light = write(&dir.join("lights/l.fits"), LIGHT, &filled(500.0));
    write(
        &dir.join("darks/master_dark.fits"),
        &[("IMAGETYP", "'Master Dark'"), ("EXPTIME", "300.0")],
        &filled(100.0),
    );
    let (p, c) = plugin(Options::default());
    assert!(c.toggle(&light));
    let off = open(&p, &light);
    assert_eq!(state(&off), Some(TURNED_OFF));
    assert_eq!(values(&off)[0], 500.0);
    assert!(!c.toggle(&light));
    assert_eq!(values(&open(&p, &light))[0], 400.0);
}

#[test]
fn calibration_frames_themselves_are_shown_as_they_are() {
    let dir = temp_dir("not-light");
    let dark = write(
        &dir.join("darks/master_dark.fits"),
        &[("IMAGETYP", "'Master Dark'"), ("EXPTIME", "300.0")],
        &filled(100.0),
    );
    write(
        &dir.join("darks/master_dark2.fits"),
        &[("IMAGETYP", "'Master Dark'"), ("EXPTIME", "300.0")],
        &filled(50.0),
    );
    let (p, _) = plugin(Options::default());
    let img = open(&p, &dark);
    assert_eq!(values(&img)[0], 100.0);
    assert_eq!(img.metadata.get(GROUP, "Frame type"), Some("dark"));
    assert_eq!(state(&img), None);
}

#[test]
fn turning_calibration_off_in_the_settings_leaves_no_trace() {
    let dir = temp_dir("off");
    let light = write(&dir.join("lights/l.fits"), LIGHT, &filled(500.0));
    write(
        &dir.join("darks/master_dark.fits"),
        &[("IMAGETYP", "'Master Dark'"), ("EXPTIME", "300.0")],
        &filled(100.0),
    );
    let (p, _) = plugin(Options {
        apply: false,
        ..Options::default()
    });
    let img = open(&p, &light);
    assert_eq!(values(&img)[0], 500.0);
    assert!(img.metadata.entries.iter().all(|(g, _, _)| g != GROUP));
}

#[test]
fn masters_in_the_library_are_used_when_nothing_is_nearby() {
    let dir = temp_dir("library");
    let light = write(&dir.join("night/M31/l.fits"), LIGHT, &filled(500.0));
    write(
        &dir.join("library/cam/300s/master_dark.fits"),
        &[("IMAGETYP", "'Master Dark'"), ("EXPTIME", "300.0")],
        &filled(100.0),
    );
    let o = Options {
        use_flat: false,
        ..Options::default()
    };
    let (p, _) = plugin(o.clone());
    assert_eq!(state(&open(&p, &light)), Some(NOT_APPLIED));
    let (p, _) = plugin(Options {
        library: vec![dir.join("library")],
        ..o
    });
    assert_eq!(values(&open(&p, &light))[0], 400.0);
}

#[test]
fn color_flats_can_be_evened_out_per_bayer_position() {
    let raw: Vec<f32> = (0..W * H)
        .map(|i| {
            if (i % W).is_multiple_of(2) {
                100.0
            } else {
                300.0
            }
        })
        .collect();
    let plain = normalize_flat(&raw, &Sub::None, W, FlatNormalize::Median, false);
    let even = normalize_flat(&raw, &Sub::None, W, FlatNormalize::Median, true);
    assert!(plain.iter().any(|&v| (v - 1.0).abs() > 0.1));
    assert!(even.iter().all(|&v| (v - 1.0).abs() < 1e-6), "{even:?}");
}

#[test]
fn flats_can_be_normalised_by_their_mean() {
    let raw = vec![1.0, 1.0, 1.0, 5.0, 1.0, 1.0, 1.0, 1.0];
    let mean = normalize_flat(&raw, &Sub::None, W, FlatNormalize::Mean, false);
    assert!((mean[0] - 1.0 / 1.5).abs() < 1e-6);
    let median = normalize_flat(&raw, &Sub::None, W, FlatNormalize::Median, false);
    assert_eq!(median[0], 1.0);
}

#[test]
fn the_reason_for_a_missing_flat_is_shown_next_to_the_applied_dark() {
    let dir = temp_dir("dark-only");
    let light = write(&dir.join("lights/l.fits"), LIGHT, &filled(500.0));
    write(
        &dir.join("darks/master_dark.fits"),
        &[("IMAGETYP", "'Master Dark'"), ("EXPTIME", "300.0")],
        &filled(100.0),
    );
    let (p, _) = plugin(Options::default());
    let img = open(&p, &light);
    assert_eq!(state(&img), Some(APPLIED));
    assert_eq!(img.metadata.get(GROUP, "Flat"), Some("no flat found"));
}
