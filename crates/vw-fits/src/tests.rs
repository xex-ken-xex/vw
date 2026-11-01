use super::*;

/// 80 文字のカード。`value` はそのまま `= ` の後ろに置く（文字列なら引用符ごと渡す）。
fn card(key: &str, value: &str) -> String {
    format!("{key:<8}= {value:>20}")
}

/// ヘッダ `cards`（SIMPLE と END は足す）とデータ `data` から FITS を作る。
fn fits(cards: &[String], data: &[u8]) -> Vec<u8> {
    hdu_bytes(&[card("SIMPLE", "T")], cards, data)
}

fn hdu_bytes(first: &[String], cards: &[String], data: &[u8]) -> Vec<u8> {
    let mut header = String::new();
    for c in first.iter().chain(cards) {
        header.push_str(&format!("{c:<80}"));
    }
    header.push_str(&format!("{:<80}", "END"));
    let mut out = header.into_bytes();
    out.resize(out.len().next_multiple_of(2880), b' ');
    out.extend_from_slice(data);
    out.resize(out.len().next_multiple_of(2880), 0);
    out
}

fn image_cards(bitpix: i64, axes: &[usize]) -> Vec<String> {
    let mut c = vec![
        card("BITPIX", &bitpix.to_string()),
        card("NAXIS", &axes.len().to_string()),
    ];
    for (i, a) in axes.iter().enumerate() {
        c.push(card(&format!("NAXIS{}", i + 1), &a.to_string()));
    }
    c
}

fn i16_bytes(v: &[i16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_be_bytes()).collect()
}

fn f32_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_be_bytes()).collect()
}

fn decode(data: &[u8]) -> VwImage {
    FitsPlugin::default().decode(data).unwrap()
}

fn decode_opts(data: &[u8], options: Options) -> VwImage {
    decode_with(data, 0, options, None, None).unwrap()
}

fn f32s(img: &VwImage) -> Vec<f32> {
    match &img.data {
        PixelData::F32(v) => v.clone(),
        PixelData::U16(v) => v.iter().map(|&x| x as f32).collect(),
        PixelData::U8(v) => v.iter().map(|&x| x as f32).collect(),
    }
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/testdata/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[test]
fn recognises_fits_by_its_first_card() {
    let p = FitsPlugin::default();
    assert!(p.probe(fits(&image_cards(8, &[]), &[]).as_slice()));
    assert!(!p.probe(b"SIMPLE  =                    F"));
    assert!(!p.probe(b"not a fits file at all, really"));
}

#[test]
fn offers_double_extensions_for_gzipped_files() {
    let p = FitsPlugin::default();
    for ext in ["fits", "fit", "fts", "fz", "fits.gz"] {
        assert!(p.extensions().contains(&ext), "{ext}");
    }
}

#[test]
fn unsigned_16bit_with_bzero_32768_stays_16bit() {
    let mut cards = image_cards(16, &[3, 1]);
    cards.push(card("BZERO", "32768"));
    let img = decode(&fits(&cards, &i16_bytes(&[-32768, 0, 32767])));
    assert!(matches!(&img.data, PixelData::U16(v) if v == &[0, 32768, 65535]));
}

#[test]
fn signed_16bit_keeps_its_negative_values() {
    let img = decode(&fits(&image_cards(16, &[3, 1]), &i16_bytes(&[-5, 0, 7])));
    assert_eq!(f32s(&img), [-5.0, 0.0, 7.0]);
}

#[test]
fn bscale_and_bzero_give_physical_values() {
    let mut cards = image_cards(16, &[2, 1]);
    cards.push(card("BSCALE", "0.5"));
    cards.push(card("BZERO", "100.0"));
    let img = decode(&fits(&cards, &i16_bytes(&[10, -10])));
    assert_eq!(f32s(&img), [105.0, 95.0]);
}

#[test]
fn every_bitpix_is_read_big_endian() {
    let img = decode(&fits(&image_cards(8, &[2, 1]), &[3, 250]));
    assert!(matches!(&img.data, PixelData::U8(v) if v == &[3, 250]));
    let data: Vec<u8> = [-70000i32, 5]
        .iter()
        .flat_map(|x| x.to_be_bytes())
        .collect();
    assert_eq!(
        f32s(&decode(&fits(&image_cards(32, &[2, 1]), &data))),
        [-70000.0, 5.0]
    );
    let data: Vec<u8> = [-3i64, 9].iter().flat_map(|x| x.to_be_bytes()).collect();
    assert_eq!(
        f32s(&decode(&fits(&image_cards(64, &[2, 1]), &data))),
        [-3.0, 9.0]
    );
    let img = decode(&fits(&image_cards(-32, &[2, 1]), &f32_bytes(&[0.25, -1.5])));
    assert_eq!(f32s(&img), [0.25, -1.5]);
    let data: Vec<u8> = [2.5f64, -0.125]
        .iter()
        .flat_map(|x| x.to_be_bytes())
        .collect();
    assert_eq!(
        f32s(&decode(&fits(&image_cards(-64, &[2, 1]), &data))),
        [2.5, -0.125]
    );
}

#[test]
fn blank_integers_and_nan_floats_are_missing() {
    let mut cards = image_cards(16, &[3, 1]);
    cards.push(card("BLANK", "-32768"));
    let v = f32s(&decode(&fits(&cards, &i16_bytes(&[1, -32768, 3]))));
    assert!(v[0] == 1.0 && v[1].is_nan() && v[2] == 3.0);
    let v = f32s(&decode(&fits(
        &image_cards(-32, &[2, 1]),
        &f32_bytes(&[f32::NAN, 2.0]),
    )));
    assert!(v[0].is_nan() && v[1] == 2.0);
}

#[test]
fn planes_of_a_cube_are_frames() {
    let p = FitsPlugin::default();
    let file = fits(&image_cards(8, &[2, 1, 3]), &[1, 2, 3, 4, 5, 6]);
    assert_eq!(p.frame_count(&file).unwrap(), 3);
    let third = p.decode_frame(&file, 2).unwrap();
    assert!(matches!(&third.data, PixelData::U8(v) if v == &[5, 6]));
    assert_eq!(third.metadata.get(GROUP, "Frame"), Some("3 / 3"));
    assert!(p.decode_frame(&file, 3).is_err());
}

#[test]
fn image_extension_is_used_when_the_primary_has_no_data() {
    let mut file = fits(
        &[
            card("BITPIX", "8"),
            card("NAXIS", "0"),
            card("OBJECT", "'M31'"),
        ],
        &[],
    );
    let mut ext = vec![card("XTENSION", "'IMAGE   '")];
    ext.extend(image_cards(8, &[2, 1]));
    ext.push(card("EXTNAME", "'SCI'"));
    file.extend(hdu_bytes(&ext, &[], &[7, 8]));
    let img = decode(&file);
    assert!(matches!(&img.data, PixelData::U8(v) if v == &[7, 8]));
    assert_eq!(img.metadata.get(GROUP, "OBJECT"), Some("M31"));
    assert_eq!(img.metadata.get("FITS HDU 1", "EXTNAME"), Some("SCI"));
}

#[test]
fn file_without_an_image_is_unsupported() {
    let file = fits(&image_cards(8, &[]), &[]);
    assert!(matches!(
        FitsPlugin::default().decode(&file),
        Err(DecodeError::Unsupported(_))
    ));
}

#[test]
fn truncated_data_is_invalid() {
    let mut file = fits(&image_cards(16, &[100, 100]), &[0; 20000]);
    file.truncate(5000);
    assert!(matches!(
        FitsPlugin::default().decode(&file),
        Err(DecodeError::Invalid(_))
    ));
    assert!(
        FitsPlugin::default()
            .decode(b"SIMPLE  =                    T")
            .is_err()
    );
}

#[test]
fn rows_follow_roworder_and_otherwise_the_setting() {
    let rows = |order: Option<&str>, flip_rows: bool| {
        let mut cards = image_cards(8, &[1, 2]);
        if let Some(order) = order {
            cards.push(card("ROWORDER", &format!("'{order}'")));
        }
        let options = Options {
            flip_rows,
            ..Options::default()
        };
        f32s(&decode_opts(&fits(&cards, &[1, 2]), options))
    };
    assert_eq!(rows(None, false), [1.0, 2.0]);
    assert_eq!(rows(None, true), [2.0, 1.0]);
    assert_eq!(rows(Some("BOTTOM-UP"), false), [2.0, 1.0]);
    assert_eq!(rows(Some("TOP-DOWN"), true), [1.0, 2.0]);
}

/// 4x4 のモザイクのヘッダ。
fn mosaic_cards(pattern: &str, x_off: i64, y_off: i64) -> Vec<String> {
    let mut cards = image_cards(8, &[4, 4]);
    cards.push(card("BAYERPAT", &format!("'{pattern}'")));
    cards.push(card("XBAYROFF", &x_off.to_string()));
    cards.push(card("YBAYROFF", &y_off.to_string()));
    cards
}

/// 偶数の行・偶数の列（RGGB の赤の位置）だけ 100、ほかは 0。
fn red_at_even() -> [u8; 16] {
    let mut px = [0u8; 16];
    for (x, y) in [(0, 0), (2, 0), (0, 2), (2, 2)] {
        px[y * 4 + x] = 100;
    }
    px
}

fn rgb_at(img: &VwImage, x: usize, y: usize) -> [f32; 3] {
    let v = f32s(img);
    let at = (y * img.width as usize + x) * 3;
    [v[at], v[at + 1], v[at + 2]]
}

#[test]
fn bayer_mosaic_becomes_rgb_with_the_right_colours() {
    let img = decode(&fits(&mosaic_cards("RGGB", 0, 0), &red_at_even()));
    assert_eq!(img.color, ColorModel::Rgb);
    assert_eq!(rgb_at(&img, 0, 0), [100.0, 0.0, 0.0]);
    assert_eq!(rgb_at(&img, 1, 1)[0], 100.0);
    // GRBG を 1 つ横にずらすと RGGB と同じ並びになる。
    let shifted = decode(&fits(&mosaic_cards("GRBG", 1, 0), &red_at_even()));
    assert_eq!(rgb_at(&shifted, 0, 0), [100.0, 0.0, 0.0]);
    // BGGR なら同じ画素が青。
    let blue = decode(&fits(&mosaic_cards("BGGR", 0, 0), &red_at_even()));
    assert_eq!(rgb_at(&blue, 0, 0), [0.0, 0.0, 100.0]);
}

#[test]
fn bayer_colours_survive_turning_upside_down() {
    let mut cards = mosaic_cards("RGGB", 0, 0);
    cards.push(card("ROWORDER", "'BOTTOM-UP'"));
    let img = decode(&fits(&cards, &red_at_even()));
    // ファイルの 0 行目（赤のある行）は、返したあと一番下の行になる。
    assert_eq!(rgb_at(&img, 0, 3), [100.0, 0.0, 0.0]);
}

#[test]
fn demosaic_can_be_turned_off() {
    let options = Options {
        demosaic: false,
        ..Options::default()
    };
    let img = decode_opts(&fits(&mosaic_cards("RGGB", 0, 0), &[0; 16]), options);
    assert_eq!(img.color, ColorModel::Gray);
}

#[test]
fn every_card_is_listed_in_file_order() {
    let mut cards = image_cards(8, &[1, 1]);
    cards.extend([
        card("OBJECT", "'NGC 7000'"),
        card("EXPTIME", "120.0 / [s]"),
        "HIERARCH ESO DET DIT = 1.5 / integration".to_owned(),
        card("LONGSTR", "'first part &'"),
        "CONTINUE  'second part'".to_owned(),
        "COMMENT first comment".to_owned(),
        "HISTORY made by a test".to_owned(),
        card("QUOTE", "'it''s'"),
    ]);
    let m = decode(&fits(&cards, &[0])).metadata;
    let keys: Vec<&str> = m.entries.iter().map(|(_, k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [
            "Size",
            "SIMPLE",
            "BITPIX",
            "NAXIS",
            "NAXIS1",
            "NAXIS2",
            "OBJECT",
            "EXPTIME",
            "HIERARCH ESO DET DIT",
            "LONGSTR",
            "COMMENT",
            "HISTORY",
            "QUOTE",
            STRETCH,
            BLACK_PERCENTILE,
            WHITE_PERCENTILE
        ]
    );
    assert_eq!(m.get(GROUP, "OBJECT"), Some("NGC 7000"));
    assert_eq!(m.get(GROUP, "EXPTIME"), Some("120.0"));
    assert_eq!(m.get(GROUP, "HIERARCH ESO DET DIT"), Some("1.5"));
    assert_eq!(m.get(GROUP, "LONGSTR"), Some("first part second part"));
    assert_eq!(m.get(GROUP, "COMMENT"), Some("first comment"));
    assert_eq!(m.get(GROUP, "QUOTE"), Some("it's"));
}

#[test]
fn how_black_and_white_were_chosen_follows_the_cards() {
    let options = Options {
        stretch: Stretch::ZScale,
        ..Options::default()
    };
    let m = decode_opts(&fits(&image_cards(8, &[1, 1]), &[0]), options).metadata;
    assert_eq!(m.get(GROUP, STRETCH), Some("zscale"));
    assert_eq!(m.get(GROUP, BLACK_PERCENTILE), None);
    let m = decode(&fits(&image_cards(8, &[1, 1]), &[0])).metadata;
    assert_eq!(m.get(GROUP, STRETCH), Some("Percentiles"));
    assert_eq!(m.get(GROUP, WHITE_PERCENTILE), Some("99.5 %"));
}

#[test]
fn metadata_is_read_from_the_header_alone() {
    let mut cards = image_cards(16, &[100, 100]);
    cards.push(card("FILTER", "'Ha'"));
    let mut file = fits(&cards, &[]);
    file.truncate(2880);
    let m = FitsPlugin::default().read_metadata(&file).unwrap();
    assert_eq!(m.get(GROUP, "FILTER"), Some("Ha"));
}

#[test]
fn whole_file_gzip_is_unpacked() {
    use std::io::Write;
    let file = fits(&image_cards(8, &[2, 1]), &[9, 8]);
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&file).unwrap();
    let img = decode(&gz.finish().unwrap());
    assert!(matches!(&img.data, PixelData::U8(v) if v == &[9, 8]));
}

#[test]
fn first_window_is_the_half_percent_range_by_default() {
    // 0..=999 の 16bit。0.5％ は 5、99.5％ は 994。
    let stored: Vec<i16> = (0..1000).map(|v: i32| (v - 32768) as i16).collect();
    let mut cards = image_cards(16, &[1000, 1]);
    cards.push(card("BZERO", "32768"));
    let img = decode(&fits(&cards, &i16_bytes(&stored)));
    assert_eq!(img.window, Some((5.0, 994.0)));
}

#[test]
fn curve_is_linear_by_default_and_follows_the_setting() {
    let file = fits(&image_cards(8, &[2, 1]), &[0, 255]);
    assert_eq!(decode(&file).curve, Curve::Linear);
    let asinh = Options {
        curve: Curve::Asinh,
        ..Options::default()
    };
    assert_eq!(decode_opts(&file, asinh).curve, Curve::Asinh);
}

#[test]
fn stretch_methods_choose_black_and_white() {
    let mut v: Vec<f32> = (0..1000).map(|i| 100.0 + (i % 10) as f32).collect();
    v[0] = 60000.0; // 明るい星
    v[1] = f32::NAN;
    let data = PixelData::F32(v);
    assert_eq!(
        stretch::window(&data, Stretch::MinMax, 0.0, 0.0),
        Some((100.0, 60000.0))
    );
    let (lo, hi) = stretch::window(&data, Stretch::Percentile, 0.5, 99.5).unwrap();
    assert!(
        (100.0..=101.0).contains(&lo) && (108.0..=109.0).contains(&hi),
        "{lo} {hi}"
    );
    // zscale は背景の付近だけを見るので、明るい星に引っ張られない。
    let (lo, hi) = stretch::window(&data, Stretch::ZScale, 0.0, 0.0).unwrap();
    assert!(lo >= 100.0 && hi <= 200.0, "{lo} {hi}");
    // 一面同じ値でも幅 1 の範囲になる。
    let flat = PixelData::U16(vec![7; 10]);
    assert_eq!(
        stretch::window(&flat, Stretch::Percentile, 0.5, 99.5),
        Some((7.0, 8.0))
    );
}

/// 圧縮した `.fz` と、同じ画素を圧縮せずに書いた `.fits` を、フレームごとに比べる。
fn same_as(compressed: &str, plain: &str) {
    let p = FitsPlugin::default();
    let (c, u) = (fixture(compressed), fixture(plain));
    let frames = p.frame_count(&u).unwrap();
    assert_eq!(p.frame_count(&c).unwrap(), frames, "{compressed}");
    for f in 0..frames {
        let a = f32s(&p.decode_frame(&c, f).unwrap());
        let b = f32s(&p.decode_frame(&u, f).unwrap());
        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(&b).enumerate() {
            assert!(
                x == y || (x.is_nan() && y.is_nan()),
                "{compressed} frame {f} at {i}: {x} vs {y}"
            );
        }
    }
}

#[test]
fn rice_and_gzip_tiles_decode_to_the_original_pixels() {
    same_as("uint16-cube-rice.fz", "uint16-cube.fits");
    same_as("uint16-cube-gzip1.fz", "uint16-cube.fits");
    same_as("uint16-cube-gzip2.fz", "uint16-cube.fits");
    same_as("int32-rice.fz", "int32.fits");
    same_as("uint8-rice.fz", "uint8.fits");
}

#[test]
fn quantized_floats_are_restored_like_cfitsio() {
    same_as(
        "float32-rice-dither1.fz",
        "float32-rice-dither1-expected.fits",
    );
    same_as(
        "float32-rice-dither2.fz",
        "float32-rice-dither2-expected.fits",
    );
    same_as(
        "float32-rice-nodither.fz",
        "float32-rice-nodither-expected.fits",
    );
}

#[test]
fn unsupported_compression_is_reported_as_unsupported() {
    let mut file = fixture("uint16-cube-rice.fz");
    let at = file.windows(10).position(|w| w == b"'RICE_1  '").unwrap();
    file[at..at + 13].copy_from_slice(b"'HCOMPRESS_1'");
    assert!(matches!(
        FitsPlugin::default().decode(&file),
        Err(DecodeError::Unsupported(m)) if m.contains("HCOMPRESS_1")
    ));
}

#[test]
fn truncated_compressed_file_is_invalid() {
    let mut file = fixture("uint16-cube-rice.fz");
    file.truncate(file.len() - 2880);
    assert!(FitsPlugin::default().decode(&file).is_err());
}

/// Primary に 1 次元のデータ（画像でない）を置き、後ろの画像の拡張に WCS、Primary に架台の
/// 向きを書いたファイル。データ部を読み飛ばして 2 つ目のヘッダに行き着くかを見る。
fn file_with_wcs_after_data() -> Vec<u8> {
    let mut primary = image_cards(8, &[5000]);
    primary.push(card("RA", "10.0"));
    primary.push(card("DEC", "20.0"));
    let mut file = fits(&primary, &[1; 5000]);
    let mut ext = vec![card("XTENSION", "'IMAGE   '")];
    ext.extend(image_cards(8, &[4, 2]));
    for (k, v) in [
        ("CTYPE1", "'RA---TAN'"),
        ("CTYPE2", "'DEC--TAN'"),
        ("CRPIX1", "2.5"),
        ("CRPIX2", "1.5"),
        ("CRVAL1", "83.8221"),
        ("CRVAL2", "-5.3911"),
        ("CDELT1", "-0.1"),
        ("CDELT2", "0.1"),
    ] {
        ext.push(card(k, v));
    }
    file.extend(hdu_bytes(&ext, &[], &[0; 8]));
    file
}

fn sky_of_bytes(name: &str, bytes: &[u8]) -> Option<Sky> {
    let path = std::env::temp_dir().join(format!("vw-fits-{}-{name}", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    let header = header_of_file(&path);
    std::fs::remove_file(&path).unwrap();
    header.unwrap().and_then(|h| h.sky())
}

#[test]
fn sky_is_read_from_the_headers_of_plain_and_gzipped_files() {
    use std::io::Write;
    let file = file_with_wcs_after_data();
    let plain = sky_of_bytes("plain.fits", &file).unwrap();
    assert!(plain.certain);
    assert!((plain.center[0] - 83.8221).abs() < 1e-9 && (plain.center[1] + 5.3911).abs() < 1e-9);
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&file).unwrap();
    let gzipped = sky_of_bytes("gz.fits.gz", &gz.finish().unwrap()).unwrap();
    assert_eq!(gzipped, plain);
}

#[test]
fn header_values_come_from_the_image_then_the_primary() {
    let path = std::env::temp_dir().join(format!("vw-fits-{}-cards.fits", std::process::id()));
    std::fs::write(&path, file_with_wcs_after_data()).unwrap();
    let header = header_of_file(&path).unwrap().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(header.get("CTYPE1"), Some("RA---TAN"));
    assert_eq!(header.get("RA"), Some("10.0"));
    assert_eq!(header.get("NAXIS1"), Some("4"));
}

#[test]
fn sky_falls_back_to_the_pointing_in_the_primary() {
    let mut cards = image_cards(8, &[4, 2]);
    cards.push(card("RA", "10.0"));
    cards.push(card("DEC", "20.0"));
    let sky = sky_of_bytes("pointing.fits", &fits(&cards, &[0; 8])).unwrap();
    assert!(!sky.certain);
    assert_eq!(sky.center, [10.0, 20.0]);
}

#[test]
fn sky_of_a_file_without_position_or_image_is_none() {
    assert_eq!(
        sky_of_bytes("none.fits", &fits(&image_cards(8, &[4, 2]), &[0; 8])),
        None
    );
    assert_eq!(
        sky_of_bytes("table.fits", &fits(&image_cards(8, &[]), &[])),
        None
    );
    let path = std::env::temp_dir().join(format!("vw-fits-{}-not.fits", std::process::id()));
    std::fs::write(&path, b"not a fits file at all").unwrap();
    assert!(header_of_file(&path).is_err());
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn huge_axes_in_the_header_are_refused_before_allocating() {
    let cards = image_cards(16, &[200_000, 200_000]);
    let err = FitsPlugin::default()
        .decode(&fits(&cards, &[0; 8]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("200000 x 200000"), "{err}");
    assert!(err.contains("image size limit"), "{err}");
}

#[test]
fn symmetric_black_and_white_are_equally_far_from_zero() {
    let v: Vec<f32> = (-50..=100).map(|i| i as f32).collect();
    let (lo, hi) = stretch::window(&PixelData::F32(v), Stretch::Symmetric, 0.0, 100.0).unwrap();
    assert_eq!((lo, hi), (-100.0, 100.0));
}

/// 8 x 1 の実数の面を `planes` 枚重ねたキューブ。面 `p` の値は `p * 10 + x`。
fn cube(planes: usize, extra: &[String]) -> Vec<u8> {
    let mut cards = image_cards(-32, &[8, 1, planes]);
    cards.extend(extra.iter().cloned());
    let values: Vec<f32> = (0..planes)
        .flat_map(|p| (0..8).map(move |x| (p * 10 + x) as f32))
        .collect();
    fits(&cards, &f32_bytes(&values))
}

#[test]
fn all_planes_of_a_cube_share_one_black_and_white() {
    let file = cube(3, &[]);
    let p = FitsPlugin::default();
    let options = Options {
        stretch: Stretch::MinMax,
        ..Options::default()
    };
    p.options.set(options);
    let windows: Vec<_> = (0..3)
        .map(|f| p.decode_frame(&file, f).unwrap().window)
        .collect();
    assert_eq!(windows, vec![Some((0.0, 27.0)); 3]);
    let first = p.decode_frame(&file, 0).unwrap();
    assert_eq!(
        first.metadata.get(GROUP, PLANES_WINDOW),
        Some(PLANES_WINDOW_SAME)
    );

    p.options.set(Options {
        same_window_for_planes: false,
        ..options
    });
    assert_eq!(p.decode_frame(&file, 2).unwrap().window, Some((20.0, 27.0)));
}

#[test]
fn a_plane_tells_its_frequency_and_radio_velocity() {
    let file = cube(
        3,
        &[
            card("CTYPE3", "'FREQ'"),
            card("CRVAL3", "1.420405752E9"),
            card("CRPIX3", "1"),
            card("CDELT3", "-1.0E5"),
            card("CUNIT3", "'Hz'"),
            card("RESTFRQ", "1.420405752E9"),
        ],
    );
    let plane = |f| {
        FitsPlugin::default()
            .decode_frame(&file, f)
            .unwrap()
            .metadata
            .get(GROUP, PLANE)
            .map(str::to_owned)
    };
    assert_eq!(plane(0).as_deref(), Some("1.420406 GHz, 0.00 km/s"));
    // 周波数が 100 kHz 下がると、およそ 21.1 km/s 遠ざかる。
    assert_eq!(plane(1).as_deref(), Some("1.420306 GHz, 21.11 km/s"));
}

#[test]
fn velocity_and_stokes_axes_are_named() {
    let mut cards = image_cards(-32, &[2, 1, 2, 4]);
    cards.extend([
        card("CTYPE3", "'VRAD'"),
        card("CRVAL3", "-5000.0"),
        card("CRPIX3", "1"),
        card("CDELT3", "2500.0"),
        card("CTYPE4", "'STOKES'"),
        card("CRVAL4", "1"),
        card("CRPIX4", "1"),
        card("CDELT4", "1"),
    ]);
    let file = fits(&cards, &f32_bytes(&[0.0; 16]));
    let plane = |f| {
        FitsPlugin::default()
            .decode_frame(&file, f)
            .unwrap()
            .metadata
            .get(GROUP, PLANE)
            .map(str::to_owned)
    };
    assert_eq!(plane(0).as_deref(), Some("-5.00 km/s, Stokes I"));
    assert_eq!(plane(7).as_deref(), Some("-2.50 km/s, Stokes V"));
}

#[test]
fn missing_values_get_the_missing_color() {
    let file = fits(&image_cards(-32, &[2, 1]), &f32_bytes(&[f32::NAN, 1.0]));
    assert_eq!(decode(&file).missing, Some(MISSING_COLOR));
}

#[test]
fn the_beam_is_turned_into_pixels_through_the_wcs() {
    let mut m = Metadata::default();
    for (k, v) in [
        ("CDELT1", "-0.001"),
        ("CDELT2", "0.001"),
        ("BMAJ", "0.004"),
        ("BMIN", "0.002"),
        ("BPA", "90.0"),
    ] {
        m.push(GROUP, k, v);
    }
    let b = radio::beam(&m).unwrap();
    // 長径は東西（PA 90°）。東は画素の x の負の向き。半径 2 画素。
    assert!(
        (b.major[0] + 2.0).abs() < 1e-9 && b.major[1].abs() < 1e-9,
        "{b:?}"
    );
    assert!(
        b.minor[0].abs() < 1e-9 && (b.minor[1].abs() - 1.0).abs() < 1e-9,
        "{b:?}"
    );
    assert!((b.bmaj_arcsec - 14.4).abs() < 1e-9);
    m.entries.retain(|(_, k, _)| k != "BMIN");
    assert_eq!(radio::beam(&m), None);
}

fn spectrum_cards(axes: &[usize]) -> Vec<String> {
    let mut c = image_cards(-32, axes);
    c.extend([
        card("CTYPE1", "'FREQ    '"),
        card("CUNIT1", "'GHz     '"),
        card("CRVAL1", "110.0"),
        card("CRPIX1", "2.0"),
        card("CDELT1", "-0.5"),
        card("RESTFRQ", "110.0E9"),
        card("BUNIT", "'K       '"),
    ]);
    c
}

#[test]
fn a_one_axis_file_is_read_as_a_spectrum() {
    let img = decode(&fits(
        &spectrum_cards(&[4]),
        &f32_bytes(&[1.0, 2.0, f32::NAN, 4.0]),
    ));
    assert_eq!((img.width, img.height), (4, 1));
    let s = img.spectrum.expect("a spectrum");
    assert_eq!(s.values.len(), 4);
    assert!(s.values[2].is_nan());
    assert_eq!(s.axis.kind, vw_core::spectrum::AxisKind::Frequency);
    assert_eq!(s.axis.value(1.0), 110e9);
    assert_eq!(s.axis.value(0.0), 110.5e9);
    assert_eq!(s.radio_velocity(110e9), Some(0.0));
    assert_eq!(s.value_unit, "K");
    assert_eq!(
        img.metadata.get(GROUP, SPECTRUM),
        Some("4 points, frequency")
    );
}

#[test]
fn a_spectrum_with_extra_axes_of_length_one_is_a_spectrum_too() {
    let img = decode(&fits(
        &spectrum_cards(&[3, 1, 1, 1]),
        &f32_bytes(&[1.0, 2.0, 3.0]),
    ));
    assert!(img.spectrum.is_some());
}

#[test]
fn images_are_not_spectra() {
    let img = decode(&fits(&image_cards(-32, &[2, 2]), &f32_bytes(&[1.0; 4])));
    assert!(img.spectrum.is_none());
}

#[test]
fn velocity_and_wavelength_axes_are_kept_in_base_units() {
    use vw_core::spectrum::AxisKind;
    let axis = |ctype: &str, unit: &str| {
        let mut c = image_cards(-32, &[2]);
        c.extend([
            card("CTYPE1", &format!("'{ctype:<8}'")),
            card("CUNIT1", &format!("'{unit:<8}'")),
            card("CRVAL1", "3.0"),
            card("CDELT1", "1.0"),
        ]);
        decode(&fits(&c, &f32_bytes(&[0.0, 1.0])))
            .spectrum
            .unwrap()
            .axis
            .clone()
    };
    let v = axis("VRAD", "km/s");
    assert_eq!((v.kind.clone(), v.reference), (AxisKind::Velocity, 3000.0));
    let w = axis("WAVE", "nm");
    assert_eq!(w.kind, AxisKind::Wavelength);
    assert!((w.reference - 3e-9).abs() < 1e-20);
    let other = axis("PIXEL", "");
    assert!(matches!(other.kind, AxisKind::Other { .. }));
}
