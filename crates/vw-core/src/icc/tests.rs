use super::*;

fn read(profile: ColorProfile) -> (Vec<(String, String)>, Option<Gamut>) {
    let bytes = profile.encode().unwrap();
    let mut m = Metadata::default();
    let gamut = push_profile(&mut m, &bytes);
    (
        m.entries.into_iter().map(|(_, k, v)| (k, v)).collect(),
        gamut,
    )
}

fn value<'a>(entries: &'a [(String, String)], key: &str) -> Option<&'a str> {
    entries
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

#[test]
fn an_srgb_profile_is_named_and_not_wider_than_srgb() {
    let (entries, gamut) = read(ColorProfile::new_srgb());
    assert_eq!(value(&entries, "Primaries"), Some("sRGB"));
    assert_eq!(value(&entries, "Tone curve"), Some("sRGB"));
    assert_eq!(value(&entries, "Color space"), Some("Rgb"));
    assert_eq!(value(&entries, "Gamut"), None);
    assert!(!gamut.unwrap().wider_than_srgb());
}

#[test]
fn display_p3_and_adobe_rgb_are_wider_than_srgb() {
    for (profile, name) in [
        (ColorProfile::new_display_p3(), "Display P3"),
        (ColorProfile::new_adobe_rgb(), "Adobe RGB (1998)"),
        (ColorProfile::new_pro_photo_rgb(), "ProPhoto RGB"),
        (ColorProfile::new_bt2020(), "BT.2020"),
    ] {
        let (entries, gamut) = read(profile);
        assert_eq!(value(&entries, "Primaries"), Some(name));
        assert!(value(&entries, "Gamut")
            .unwrap()
            .starts_with("wider than sRGB"));
        assert!(gamut.unwrap().wider_than_srgb());
    }
}

#[test]
fn a_pure_p3_green_is_outside_srgb_but_white_and_gray_are_inside() {
    let p3 = known()
        .into_iter()
        .find(|g| g.name == "Display P3")
        .unwrap();
    let green = p3.to_linear_srgb([0.0, 1.0, 0.0]);
    assert!(green.iter().any(|&v| v < -0.01), "{green:?}");
    for v in [1.0, 0.5, 0.0] {
        let gray = p3.to_linear_srgb([v, v, v]);
        assert!(
            gray.iter().all(|&c| (-0.002..=1.002).contains(&c)),
            "{gray:?}"
        );
    }
}

#[test]
fn the_known_srgb_matrix_maps_white_to_white() {
    let srgb = known().into_iter().find(|g| g.name == "sRGB").unwrap();
    let white = srgb.to_linear_srgb([1.0, 1.0, 1.0]);
    assert!(white.iter().all(|&c| (c - 1.0).abs() < 0.002), "{white:?}");
    let red = srgb.to_linear_srgb([1.0, 0.0, 0.0]);
    assert!((red[0] - 1.0).abs() < 0.002 && red[1].abs() < 0.002 && red[2].abs() < 0.002);
}

#[test]
fn a_broken_profile_is_reported_without_a_gamut() {
    let mut m = Metadata::default();
    assert_eq!(push_profile(&mut m, b"not an icc profile"), None);
    assert!(m.entries[0].2.starts_with("unreadable"));
}

#[test]
fn cicp_codes_are_named() {
    let mut m = Metadata::default();
    push_cicp(&mut m, "PNG", [12, 13, 0], true);
    let values: Vec<&str> = m.entries.iter().map(|(_, _, v)| v.as_str()).collect();
    assert_eq!(
        values,
        [
            "Display P3 (SMPTE EG 432-1) (12)",
            "sRGB (13)",
            "Identity (RGB) (0)",
            "full"
        ]
    );
}

#[test]
fn cicp_gives_a_gamut_only_for_sdr_curves() {
    assert_eq!(cicp_gamut(12, 13).unwrap().name, "Display P3");
    assert_eq!(cicp_gamut(9, 16), None);
    assert_eq!(cicp_gamut(200, 13), None);
}
