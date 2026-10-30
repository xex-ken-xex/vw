use super::*;

/// ファイルごとに (種類, キー, 値) を並べたカタログ。
fn catalog(files: &[&[(&str, &str, &str)]]) -> Catalog {
    let mut c = Catalog::default();
    for (n, meta) in files.iter().enumerate() {
        let mut ids = Vec::new();
        for &(kind, key, value) in meta.iter() {
            ids.push(c.meta.len() as u32);
            c.meta.push((kind.into(), key.into(), value.into()));
        }
        c.files.push(FileEntry {
            path: format!("{n}.jpg"),
            size: 0,
            modified: String::new(),
            meta: ids,
        });
    }
    c
}

const UNITS: [&str; 3] = [" h", " m", " s"];

const RULES: Rules = Rules {
    gap_hours: 18.0,
    distance_km: 100.0,
};

fn all(c: &Catalog) -> Vec<usize> {
    (0..c.files.len()).collect()
}

fn files_of(groups: &[Group]) -> Vec<Vec<usize>> {
    groups.iter().map(|g| g.files.clone()).collect()
}

#[test]
fn events_split_at_a_long_pause_and_keep_overnight_trips_together() {
    let c = catalog(&[
        &[("exif", "DateTimeOriginal", "2024-05-03 09:00:00")],
        &[("exif", "DateTimeOriginal", "2024-05-01 10:00:00")],
        &[("exif", "DateTimeOriginal", "2024-05-01 21:00:00")],
        &[("exif", "DateTimeOriginal", "2024-05-02 07:30:00")],
        &[("exif", "DateTimeOriginal", "2024-05-02 18:00:00")],
        &[("exif", "DateTimeOriginal", "2024-06-10 12:00:00")],
    ]);
    let groups = group(&c, &all(&c), &GroupBy::Events, RULES);
    assert_eq!(files_of(&groups), [vec![1, 2, 3, 4, 0], vec![5]]);
    assert_eq!(span_text(groups[0].span.unwrap()), "2024-05-01 – 05-03");
}

#[test]
fn events_split_when_the_next_shot_is_far_away() {
    let at = |t, lat, lon| -> [(&'static str, &'static str, &'static str); 3] {
        [
            ("exif", "DateTimeOriginal", t),
            ("location", "Latitude", lat),
            ("location", "Longitude", lon),
        ]
    };
    let tokyo1 = at("2024-05-01 10:00:00", "35.68", "139.76");
    let tokyo2 = at("2024-05-01 11:00:00", "35.66", "139.70");
    let osaka = at("2024-05-01 14:00:00", "34.69", "135.50");
    let c = catalog(&[
        &tokyo1,
        &tokyo2,
        &[("exif", "DateTimeOriginal", "2024-05-01 13:00:00")],
        &osaka,
    ]);
    let groups = group(&c, &all(&c), &GroupBy::Events, RULES);
    assert_eq!(files_of(&groups), [vec![0, 1, 2], vec![3]]);
    let off = Rules {
        distance_km: 0.0,
        ..RULES
    };
    assert_eq!(group(&c, &all(&c), &GroupBy::Events, off).len(), 1);
}

#[test]
fn files_without_a_date_go_to_the_last_group() {
    let c = catalog(&[&[], &[("exif", "DateTimeOriginal", "2024-05-01 10:00:00")]]);
    let groups = group(&c, &all(&c), &GroupBy::Events, RULES);
    assert_eq!(files_of(&groups), [vec![1], vec![0]]);
    assert!(groups[1].is_rest());
    assert!(!groups[0].is_rest());
}

#[test]
fn nights_run_from_noon_to_noon_in_local_time() {
    // 日本（東経 139.7 度）で 21 時から翌 4 時まで。世界時では 12 時をまたぐ。
    let utc = |t| [("fits", "DATE-OBS", t), ("fits", "SITELONG", "139 42 00")];
    let c = catalog(&[
        &utc("2024-01-01T12:00:00"),
        &utc("2024-01-01T19:00:00.5"),
        &utc("2024-01-02T12:30:00"),
        &[("fits", "DATE-LOC", "2024-01-01T23:00:00")],
    ]);
    let groups = group(&c, &all(&c), &GroupBy::Nights, RULES);
    let titles: Vec<&str> = groups.iter().map(|g| g.title.as_str()).collect();
    assert_eq!(titles, ["2024-01-01", "2024-01-02"]);
    assert_eq!(files_of(&groups), [vec![0, 1, 3], vec![2]]);
}

#[test]
fn values_group_with_total_exposure_and_the_most_common_place() {
    let frame = |object, exp| {
        [
            ("fits", "OBJECT", object),
            ("fits", "EXPTIME", exp),
            ("location", "Town", "Nobeyama"),
        ]
    };
    let c = catalog(&[
        &frame("M42", "300.0"),
        &frame("M31", "120"),
        &frame("M42", "300"),
        &[("exif", "Model", "X")],
    ]);
    let by = GroupBy::from_setting("fits:OBJECT");
    let groups = group(&c, &all(&c), &by, RULES);
    let titles: Vec<&str> = groups.iter().map(|g| g.title.as_str()).collect();
    assert_eq!(titles, ["M31", "M42", ""]);
    assert_eq!(groups[1].files, [0, 2]);
    assert_eq!(groups[1].exposure, Some(600.0));
    assert_eq!(exposure_text(600.0, UNITS), "10 m 00 s");
    assert_eq!(groups[1].place.as_deref(), Some("Nobeyama"));
    assert_eq!(groups[2].exposure, None);
}

#[test]
fn key_matching_ignores_case_and_any_kind_when_none_is_given() {
    let c = catalog(&[&[("fits", "FILTER", "Ha")], &[("fits", "filter", "Ha")]]);
    let by = GroupBy::from_setting("Filter");
    assert_eq!(files_of(&group(&c, &all(&c), &by, RULES)), [vec![0, 1]]);
}

#[test]
fn only_the_given_files_are_grouped() {
    let c = catalog(&[&[("fits", "FILTER", "Ha")], &[("fits", "FILTER", "OIII")]]);
    let by = GroupBy::from_setting("fits:FILTER");
    assert_eq!(files_of(&group(&c, &[1], &by, RULES)), [vec![1]]);
}

#[test]
fn group_by_round_trips_through_the_setting_text() {
    for by in [
        GroupBy::None,
        GroupBy::Events,
        GroupBy::Nights,
        GroupBy::Key {
            kind: "fits".into(),
            key: "OBJECT".into(),
        },
    ] {
        assert_eq!(GroupBy::from_setting(&by.to_setting()), by);
    }
}

#[test]
fn spans_and_exposures_read_naturally() {
    let t = |s| parse_time(s).unwrap();
    assert_eq!(
        span_text((t("2024-05-01 10:20:00"), t("2024-05-01 18:05:00"))),
        "2024-05-01 10:20 – 18:05"
    );
    assert_eq!(
        span_text((t("2024-05-01 10:20:00"), t("2024-05-01 10:20:00"))),
        "2024-05-01 10:20"
    );
    assert_eq!(
        span_text((t("2024-12-31 10:00:00"), t("2025-01-02 10:00:00"))),
        "2024-12-31 – 2025-01-02"
    );
    assert_eq!(exposure_text(7500.0, UNITS), "2 h 05 m");
    assert_eq!(exposure_text(45.2, UNITS), "45 s");
}

#[test]
fn site_longitude_reads_decimal_and_sexagesimal() {
    assert_eq!(parse_degrees("139.5"), Some(139.5));
    assert_eq!(parse_degrees("+139 30 00"), Some(139.5));
    assert_eq!(parse_degrees("-70:45:00"), Some(-70.75));
    assert_eq!(parse_degrees("east"), None);
}
