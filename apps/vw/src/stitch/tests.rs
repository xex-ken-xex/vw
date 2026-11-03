use super::*;
use crate::test_util::{TempDir, png_registry, write_png};

/// 120 × 80 の、どの画素も違う色の絵（RGB）。
fn scene() -> Vec<u8> {
    (0..120 * 80)
        .flat_map(|i| {
            let (x, y) = (i % 120, i / 120);
            [(x * 2) as u8, (y * 3) as u8, ((x * 7 + y * 5) % 256) as u8]
        })
        .collect()
}

/// `scene` の (x, y) から切り出した `w` × `h` を PNG に書く。
fn crop(dir: &Path, name: &str, x: usize, y: usize, w: usize, h: usize) -> PathBuf {
    let s = scene();
    let rgb: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let at = ((y + i / w) * 120 + x + i % w) * 3;
            [s[at], s[at + 1], s[at + 2]]
        })
        .collect();
    let path = dir.join(name);
    write_png(&path, w as u32, h as u32, &rgb);
    path
}

fn shift(x: f64, y: f64) -> Affine {
    Affine {
        c: x,
        f: y,
        ..Affine::IDENTITY
    }
}

fn source(path: PathBuf, size: [usize; 2], placed: Affine, how: How) -> Source {
    Source {
        path,
        size,
        placed: placed.into(),
        how,
        correlation: (how == How::Matched).then_some(0.97),
        gain: 1.0,
        wcs: None,
    }
}

fn plan(sources: Vec<Source>) -> Plan {
    Plan {
        sources,
        projection: Projection::Affine,
        blend: Blend::Feather,
        matched_brightness: false,
        max_pixels: 1_000_000,
        color: true,
        depth: Depth::U8,
        window: None,
        curve: Curve::default(),
    }
}

fn two_halves(tmp: &TempDir) -> Plan {
    let a = crop(tmp.path(), "a.png", 0, 0, 80, 80);
    let b = crop(tmp.path(), "b.png", 40, 0, 80, 80);
    plan(vec![
        source(a, [80, 80], Affine::IDENTITY, How::Reference),
        source(b, [80, 80], shift(40.0, 0.0), How::Matched),
    ])
}

#[test]
fn two_overlapping_crops_are_rendered_back_into_the_scene() {
    let tmp = TempDir::new();
    let (image, read) = render(&two_halves(&tmp), &png_registry()).unwrap();
    assert!(read > 0);
    assert_eq!((image.width, image.height), (120, 80));
    assert_eq!(image.color, ColorModel::Rgba);
    let PixelData::U8(got) = &image.data else {
        panic!("8-bit sources give an 8-bit image");
    };
    let want = scene();
    for p in 0..120 * 80 {
        for c in 0..3 {
            let (g, w) = (got[p * 4 + c], want[p * 3 + c]);
            assert!(g.abs_diff(w) <= 1, "pixel {p} channel {c}: {g} {w}");
        }
        assert_eq!(got[p * 4 + 3], 255);
    }
}

#[test]
fn what_no_source_covers_is_transparent() {
    let tmp = TempDir::new();
    let a = crop(tmp.path(), "a.png", 0, 0, 60, 40);
    let b = crop(tmp.path(), "b.png", 40, 30, 60, 40);
    let p = plan(vec![
        source(a, [60, 40], Affine::IDENTITY, How::Reference),
        source(b, [60, 40], shift(40.0, 30.0), How::Matched),
    ]);
    let (image, _) = render(&p, &png_registry()).unwrap();
    assert_eq!((image.width, image.height), (100, 70));
    let PixelData::U8(v) = &image.data else {
        panic!("8-bit");
    };
    let alpha = |x: usize, y: usize| v[(y * 100 + x) * 4 + 3];
    assert_eq!(alpha(99, 0), 0, "top right is outside both");
    assert_eq!(alpha(0, 69), 0, "bottom left is outside both");
    assert_eq!(alpha(50, 35), 255);
}

#[test]
fn a_source_whose_size_changed_is_refused() {
    let tmp = TempDir::new();
    let mut p = two_halves(&tmp);
    p.sources[1].size = [90, 80];
    let err = render(&p, &png_registry()).unwrap_err();
    assert!(err.contains("size changed"), "{err}");
}

#[test]
fn the_metadata_lists_where_each_source_went_and_how() {
    let tmp = TempDir::new();
    let p = two_halves(&tmp);
    let (image, _) = render(&p, &png_registry()).unwrap();
    let m = &image.metadata;
    assert_eq!(m.get(GROUP, IMAGES), Some("2"));
    assert_eq!(m.get(GROUP, SIZE), Some("120 x 80"));
    assert_eq!(m.get(GROUP, SCALE), Some("1:1"));
    let b = m.get(GROUP, "b.png").unwrap();
    assert!(
        b.starts_with("x 40, y 0, 80 x 80, 0.00°, ×1.000, r 0.97"),
        "{b}"
    );
    assert!(
        b.ends_with(&format!("{HOW_MARK}{}", How::Matched.title())),
        "{b}"
    );
    let a = m.get(GROUP, "a.png").unwrap();
    assert!(a.ends_with(How::Reference.title()), "{a}");
}

#[test]
fn a_remembered_plan_is_named_after_its_first_image_and_the_rest() {
    let tmp = TempDir::new();
    let path = remember(two_halves(&tmp)).unwrap();
    assert!(is_stitched(&path));
    assert!(split(&path).is_some());
    assert_eq!(title(&path).as_deref(), Some("a +1"));
    assert!(!is_stitched(&tmp.path().join("a.png")));
}

fn wcs_cards(crpix: [f64; 2]) -> Vec<(String, String)> {
    [
        ("CTYPE1", "RA---TAN".to_owned()),
        ("CTYPE2", "DEC--TAN".to_owned()),
        ("CRPIX1", crpix[0].to_string()),
        ("CRPIX2", crpix[1].to_string()),
        ("CRVAL1", "10".to_owned()),
        ("CRVAL2", "20".to_owned()),
        ("CD1_1", "-0.001".to_owned()),
        ("CD1_2", "0".to_owned()),
        ("CD2_1", "0".to_owned()),
        ("CD2_2", "0.001".to_owned()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect()
}

#[test]
fn the_stitched_image_carries_the_wcs_of_its_reference_moved_to_its_place() {
    let to_fits = shift(1.0, 1.0);
    let mut a = source(
        "a.fits".into(),
        [100, 100],
        shift(30.0, 20.0),
        How::Reference,
    );
    a.wcs = Some((wcs_cards([50.0, 50.0]), to_fits));
    let b = source("b.fits".into(), [100, 100], Affine::IDENTITY, How::Matched);
    let p = plan(vec![a, b]);
    let grid = p.grid().unwrap();
    let (cards, from) = p.wcs(&grid).unwrap();
    assert_eq!(from.path, PathBuf::from("a.fits"));
    let value = |k: &str| -> f64 { lookup(&cards)(k).unwrap().parse().unwrap() };
    assert!((value("CRPIX1") - 80.0).abs() < 1e-6, "{cards:?}");
    assert!((value("CRPIX2") - 70.0).abs() < 1e-6, "{cards:?}");
    assert!((value("CRVAL1") - 10.0).abs() < 1e-9);
}

#[test]
fn the_sky_chart_gets_the_outer_frame_and_one_outline_per_source() {
    let mut a = source(
        "a.fits".into(),
        [100, 100],
        Affine::IDENTITY,
        How::Reference,
    );
    a.wcs = Some((wcs_cards([50.0, 50.0]), shift(1.0, 1.0)));
    let b = source("b.fits".into(), [100, 100], shift(60.0, 0.0), How::Matched);
    let p = plan(vec![a, b]);
    let items = sky_items(&p, Path::new("a.fits?stitch=0"));
    let kinds: Vec<crate::sky_window::Kind> = items.iter().map(|i| i.kind).collect();
    use crate::sky_window::Kind;
    assert_eq!(kinds, vec![Kind::Stitched, Kind::Part, Kind::Part]);
}

#[test]
fn without_any_wcs_nothing_goes_on_the_sky_chart() {
    let tmp = TempDir::new();
    let p = two_halves(&tmp);
    assert!(sky_items(&p, Path::new("a.png?stitch=0")).is_empty());
}

#[test]
fn a_tiny_negative_angle_is_written_as_zero() {
    assert_eq!(fixed(-0.001, 2), "0.00");
    assert_eq!(fixed(-0.2, 0), "0");
    assert_eq!(fixed(-1.5, 2), "-1.50");
}
