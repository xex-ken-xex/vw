use std::path::{Path, PathBuf};

use vw_core::{DecoderPlugin, PixelData, VwImage};

use super::frame::{is_calibration_folder, seconds};
use super::*;
use crate::{FitsPlugin, SharedOptions};

const W: usize = 6;
const H: usize = 4;

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
    assert_eq!(
        img.metadata.get(GROUP, "Formula"),
        Some("(light - dark) / flat")
    );
    assert_eq!(
        img.metadata.get(GROUP, "Subtracted (median)"),
        Some("100.0")
    );
    assert_eq!(
        img.metadata.get(GROUP, "Flat (1-99 %)"),
        Some("0.500 - 1.000")
    );
    assert_eq!(
        img.metadata.get(GROUP, "Light median"),
        Some("1100.0 -> 1000.0")
    );
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
    assert!(
        img.metadata
            .get(GROUP, "Dark")
            .unwrap()
            .contains("no master")
    );
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
        synthetic_bias: Some(100.0),
        ..Options::default()
    });
    let img = open(&p, &light);
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

#[test]
fn a_flat_is_not_used_when_nothing_can_be_subtracted_from_the_light_first() {
    let dir = temp_dir("flat-alone");
    let light = write(&dir.join("lights/l.fits"), LIGHT, &filled(1100.0));
    write(
        &dir.join("flats/master_flat.fits"),
        &[("IMAGETYP", "'Master Flat'"), ("FILTER", "'Ha'")],
        &filled(2.0),
    );
    let (p, _) = plugin(Options::default());
    let img = open(&p, &light);
    assert_eq!(state(&img), Some(NOT_APPLIED));
    assert_eq!(values(&img)[0], 1100.0);
    assert!(
        img.metadata
            .get(GROUP, "Flat")
            .unwrap()
            .contains("subtract")
    );
    let (p, _) = plugin(Options {
        synthetic_bias: Some(100.0),
        ..Options::default()
    });
    assert_eq!(state(&open(&p, &light)), Some(APPLIED));
}

#[test]
fn a_dark_chosen_by_hand_is_used_for_the_lights_of_that_folder_until_set_back_to_auto() {
    let dir = temp_dir("pick");
    let light = write(&dir.join("lights/l.fits"), LIGHT, &filled(500.0));
    let other = write(&dir.join("lights/l2.fits"), LIGHT, &filled(500.0));
    let dark = |name: &str, temp: &str, v: f32| {
        write(
            &dir.join("darks").join(name),
            &[
                ("IMAGETYP", "'Master Dark'"),
                ("EXPTIME", "300.0"),
                ("CCD-TEMP", temp),
                ("GAIN", "100"),
            ],
            &filled(v),
        )
    };
    dark("near.fits", "-10.0", 10.0);
    let far = dark("far.fits", "-11.5", 20.0);
    let (p, calibration) = plugin(Options {
        use_flat: false,
        ..Options::default()
    });

    let img = open(&p, &light);
    assert_eq!(values(&img)[0], 490.0);
    let candidates: Vec<&str> = img
        .metadata
        .entries
        .iter()
        .filter(|(g, k, _)| g == GROUP && k == Role::Dark.candidate_key())
        .map(|(_, _, v)| v.as_str())
        .collect();
    assert_eq!(candidates.len(), 2, "{candidates:?}");
    assert!(candidates[0].ends_with("near.fits"), "{candidates:?}");
    assert_eq!(img.metadata.get(GROUP, CHOSEN), None);

    calibration.choose(&light, Role::Dark, Some(far));
    for path in [&light, &other] {
        let img = open(&p, path);
        assert_eq!(values(&img)[0], 480.0);
        assert_eq!(img.metadata.get(GROUP, CHOSEN), Some("Dark"));
    }

    calibration.choose(&light, Role::Dark, None);
    assert_eq!(values(&open(&p, &light))[0], 490.0);
}

#[test]
fn a_chosen_master_that_no_longer_fits_falls_back_to_the_automatic_one() {
    let dir = temp_dir("pick-gone");
    let light = write(&dir.join("lights/l.fits"), LIGHT, &filled(500.0));
    write(
        &dir.join("darks/d.fits"),
        &[
            ("IMAGETYP", "'Master Dark'"),
            ("EXPTIME", "300.0"),
            ("GAIN", "100"),
        ],
        &filled(10.0),
    );
    let (p, calibration) = plugin(Options {
        use_flat: false,
        ..Options::default()
    });
    calibration.choose(&light, Role::Dark, Some(dir.join("darks/gone.fits")));
    let img = open(&p, &light);
    assert_eq!(values(&img)[0], 490.0);
    assert_eq!(img.metadata.get(GROUP, CHOSEN), None);
}

fn building(cache: &Path) -> Options {
    Options {
        cache_dir: Some(cache.to_path_buf()),
        wait_for_masters: true,
        ..Options::default()
    }
}

fn single_darks(dir: &Path, count: usize) {
    for i in 0..count {
        let mut v = filled(100.0 + i as f32 % 2.0);
        if i == 0 {
            v[3] = 10_000.0; // 宇宙線
        }
        write(
            &dir.join(format!("darks/dark_{i:04}.fits")),
            &[
                ("IMAGETYP", "'DARK'"),
                ("EXPTIME", "300.0"),
                ("CCD-TEMP", "-10.2"),
            ],
            &v,
        );
    }
}

#[test]
fn single_frames_are_stacked_into_a_master_that_rejects_outliers() {
    let dir = temp_dir("stacked");
    let light = write(&dir.join("lights/l.fits"), LIGHT, &filled(500.0));
    single_darks(&dir, 5);
    let (p, _) = plugin(building(&dir.join("cache")));
    let img = open(&p, &light);
    assert_eq!(state(&img), Some(APPLIED));
    let v = values(&img);
    // 5 枚の 100 と 101 の σ クリップ平均。宇宙線の 1 枚の値は除かれる。
    assert!(v.iter().all(|x| (x - 399.6).abs() < 0.5), "{v:?}");
    let note = img.metadata.get(GROUP, BUILT).unwrap();
    assert!(note.contains("master dark from 5 single frames"), "{note}");
    let made: Vec<_> = std::fs::read_dir(dir.join("cache"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(made.len(), 1, "{made:?}");
    assert!(made[0].starts_with(stack::PREFIX));
}

#[test]
fn a_built_master_is_used_once_the_background_work_is_done() {
    let dir = temp_dir("background");
    let light = write(&dir.join("lights/l.fits"), LIGHT, &filled(500.0));
    single_darks(&dir, 3);
    let (p, calibration) = plugin(Options {
        wait_for_masters: false,
        ..building(&dir.join("cache"))
    });
    let before = calibration.generation();
    let first = open(&p, &light);
    assert_eq!(state(&first), Some(NOT_APPLIED));
    assert!(
        first
            .metadata
            .get(GROUP, BUILT)
            .unwrap()
            .starts_with("building")
    );
    let start = std::time::Instant::now();
    while calibration.generation() == before {
        assert!(start.elapsed().as_secs() < 10, "the master was not built");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(state(&open(&p, &light)), Some(APPLIED));
}

#[test]
fn the_name_of_a_built_master_does_not_depend_on_the_build_of_vw() {
    // FNV-1a の決まった値（"abc" → e71fa2190541574b）。
    let mut h = stack::Fnv::default();
    h.write(b"abc");
    assert_eq!(h.0, 0xe71f_a219_0541_574b);
}

#[test]
fn masters_are_built_one_at_a_time() {
    let dir = temp_dir("one-at-a-time");
    let light = write(&dir.join("lights/l.fits"), LIGHT, &filled(500.0));
    single_darks(&dir, 3);
    let (p, calibration) = plugin(Options {
        wait_for_masters: false,
        ..building(&dir.join("cache"))
    });
    // ほかのマスターを作っている間は、順番を待つ。
    let turn = calibration.0.build_turn.lock().unwrap();
    let before = calibration.generation();
    open(&p, &light);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(
        calibration.generation(),
        before,
        "built while another was building"
    );
    drop(turn);
    let start = std::time::Instant::now();
    while calibration.generation() == before {
        assert!(start.elapsed().as_secs() < 10, "the master was not built");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(state(&open(&p, &light)), Some(APPLIED));
}

#[test]
fn adding_a_frame_gives_a_new_master() {
    let dir = temp_dir("added");
    single_darks(&dir, 3);
    let frames = |dir: &Path| -> Vec<Frame> {
        let mut index = search::Index::default();
        index.frames_near(
            &dir.join("lights/l.fits"),
            &Places {
                nearby: true,
                library: &[],
            },
        )
    };
    let name = |frames: &[Frame]| stack::groups(frames)[0].file_name(3.0);
    let before = name(&frames(&dir));
    single_darks(&dir, 4);
    assert_ne!(name(&frames(&dir)), before);
}

#[test]
fn singles_with_their_own_master_are_not_stacked_again() {
    let dir = temp_dir("has-master");
    single_darks(&dir, 3);
    write(
        &dir.join("darks/master_dark.fits"),
        &[
            ("IMAGETYP", "'Master Dark'"),
            ("EXPTIME", "300.0"),
            ("CCD-TEMP", "-10.0"),
        ],
        &filled(100.0),
    );
    let mut index = search::Index::default();
    let frames = index.frames_near(
        &dir.join("lights/l.fits"),
        &Places {
            nearby: true,
            library: &[],
        },
    );
    assert!(stack::groups(&frames).is_empty());
}

#[test]
fn single_flats_of_different_brightness_are_evened_out_before_stacking() {
    let dir = temp_dir("flats");
    let light = write(&dir.join("lights/l.fits"), LIGHT, &filled(1100.0));
    write(
        &dir.join("darks/master_dark.fits"),
        &[
            ("IMAGETYP", "'Master Dark'"),
            ("EXPTIME", "300.0"),
            ("CCD-TEMP", "-10.0"),
        ],
        &filled(100.0),
    );
    for (i, level) in [1000.0f32, 2000.0, 1500.0].into_iter().enumerate() {
        let mut flat = filled(level);
        flat[0] = level / 2.0;
        write(
            &dir.join(format!("flats/flat_{i}.fits")),
            &[("IMAGETYP", "'FLAT'"), ("FILTER", "'Ha'")],
            &flat,
        );
    }
    let (p, _) = plugin(building(&dir.join("cache")));
    let v = values(&open(&p, &light));
    assert!((v[1] - 1000.0).abs() < 1e-2, "{v:?}");
    assert!((v[0] - 2000.0).abs() < 1e-2, "{v:?}");
}

#[test]
fn clipped_mean_drops_values_far_from_the_median() {
    let mut v = [10.0, 11.0, 9.0, 10.0, 500.0];
    assert_eq!(stack::clipped_mean(&mut v, 3.0), 10.0);
    assert_eq!(stack::clipped_mean(&mut [1.0, 3.0], 3.0), 2.0);
    assert_eq!(stack::clipped_mean(&mut [7.0, 7.0, 7.0, 90.0], 3.0), 7.0);
    assert!(stack::clipped_mean(&mut [], 3.0).is_nan());
}

fn hot_dark(dir: &Path) {
    let mut dark = filled(100.0);
    dark[7] = 5_100.0;
    write(
        &dir.join("darks/master_dark.fits"),
        &[
            ("IMAGETYP", "'Master Dark'"),
            ("EXPTIME", "300.0"),
            ("CCD-TEMP", "-10.0"),
        ],
        &dark,
    );
}

#[test]
fn hot_pixels_found_in_the_dark_are_filled_from_their_neighbours() {
    let dir = temp_dir("hot");
    let mut lit = filled(500.0);
    lit[7] = 5_700.0; // ダークより少し明るく残るホットピクセル
    let light = write(&dir.join("lights/l.fits"), LIGHT, &lit);
    hot_dark(&dir);
    let (p, _) = plugin(Options {
        hot_pixels: true,
        ..Options::default()
    });
    let img = open(&p, &light);
    assert_eq!(values(&img)[7], 400.0);
    assert_eq!(
        img.metadata.get(GROUP, HOT_PIXELS),
        Some("1 filled (3 sigma)")
    );
    let (p, _) = plugin(Options::default());
    assert_eq!(values(&open(&p, &light))[7], 600.0);
}

#[test]
fn hot_pixels_can_be_left_for_one_image() {
    let dir = temp_dir("hot-off");
    let light = write(&dir.join("lights/l.fits"), LIGHT, &filled(500.0));
    hot_dark(&dir);
    let (p, calibration) = plugin(Options {
        hot_pixels: true,
        ..Options::default()
    });
    assert!(calibration.toggle_hot_pixels(&light));
    let img = open(&p, &light);
    assert_eq!(img.metadata.get(GROUP, HOT_PIXELS), Some(HOT_PIXELS_OFF));
    assert!(!calibration.toggle_hot_pixels(&light));
}

#[test]
fn a_bright_corner_of_the_dark_is_not_taken_for_hot_pixels() {
    // 右上が明るくなるアンプグローに、ホットピクセルが 1 つ。
    let (w, h) = (16, 12);
    let mut dark: Vec<f32> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            100.0 + 40.0 * (x + (h as f32 - y)) + (i % 3) as f32
        })
        .collect();
    dark[5 * w + 4] += 3_000.0;
    assert_eq!(hot_pixels(&dark, w, 1, 3.0), vec![5 * w + 4]);
}

#[test]
fn hot_pixels_on_a_colour_mosaic_take_neighbours_of_the_same_colour() {
    let mut v = vec![1.0, 9.0, 1.0, 9.0, 2.0, 8.0, 2.0, 8.0, 1.0, 9.0, 100.0, 9.0];
    // 4 x 3 のモザイクの (2, 2)。同じ色は 2 画素おきの (0, 2) と (2, 0)、(0, 0)。
    fill_hot_pixels(&mut v, 4, &[10], 2);
    assert_eq!(v[10], 1.0);
}

#[test]
fn clearing_the_cache_removes_only_the_built_masters() {
    let dir = temp_dir("clear");
    let light = write(&dir.join("lights/l.fits"), LIGHT, &filled(500.0));
    single_darks(&dir, 3);
    let cache = dir.join("cache");
    let (p, calibration) = plugin(building(&cache));
    open(&p, &light);
    std::fs::write(cache.join("notes.txt"), "mine").unwrap();
    assert_eq!(calibration.clear_cache(), Ok(1));
    assert!(cache.join("notes.txt").exists());
}
