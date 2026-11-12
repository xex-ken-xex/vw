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
    decode_with(data, 0, options, None, None, None).unwrap()
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

/// JWST の形: データのない Primary と、SCI（2 x 1、実数）・ERR（2 x 1、実数）・DQ（2 x 1、整数）、
/// 最後に表（BINTABLE、選べない）。
fn jwst_like() -> Vec<u8> {
    let mut file = fits(&[card("BITPIX", "8"), card("NAXIS", "0")], &[]);
    for (name, bitpix, data) in [
        ("SCI", -32, f32_bytes(&[1.5, 2.5])),
        ("ERR", -32, f32_bytes(&[0.1, 0.2])),
        ("DQ", 16, i16_bytes(&[0, 4])),
    ] {
        let mut ext = vec![card("XTENSION", "'IMAGE   '")];
        ext.extend(image_cards(bitpix, &[2, 1]));
        ext.push(card("EXTNAME", &format!("'{name}'")));
        file.extend(hdu_bytes(&ext, &[], &data));
    }
    let table = [
        card("XTENSION", "'BINTABLE'"),
        card("BITPIX", "8"),
        card("NAXIS", "2"),
        card("NAXIS1", "4"),
        card("NAXIS2", "1"),
        card("PCOUNT", "0"),
        card("GCOUNT", "1"),
        card("TFIELDS", "1"),
        card("TFORM1", "'1E      '"),
        card("EXTNAME", "'ASDF    '"),
    ];
    file.extend(hdu_bytes(&table, &[], &[0, 0, 0, 0]));
    file
}

#[test]
fn every_image_and_table_hdu_can_be_chosen() {
    let parts = parts(&jwst_like()).unwrap();
    let names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["1 SCI", "2 ERR", "3 DQ", "4 ASDF"]);
    assert_eq!(parts[3].detail(), "BINTABLE 1 column x 1 row");
}

#[test]
fn the_chosen_hdu_is_shown_with_its_own_cards() {
    let file = jwst_like();
    let plugin = FitsPlugin::default();
    let choices = Choices::default();
    let plugin = plugin.with_choices(choices.clone());
    assert_eq!(f32s(&plugin.decode(&file).unwrap()), [1.5, 2.5]);
    choices.set(&file, 3usize);
    let img = plugin.decode(&file).unwrap();
    assert_eq!(f32s(&img), [0.0, 4.0]);
    assert_eq!(img.metadata.get(GROUP, HDU), Some("3 DQ"));
    assert_eq!(img.metadata.get("FITS HDU 3", "EXTNAME"), Some("DQ"));
}

#[test]
fn a_choice_that_is_neither_an_image_nor_a_table_falls_back_to_the_first_image() {
    let file = jwst_like();
    let choices = Choices::default();
    let plugin = FitsPlugin::default().with_choices(choices.clone());
    choices.set(&file, 9usize);
    assert_eq!(f32s(&plugin.decode(&file).unwrap()), [1.5, 2.5]);
}

#[test]
fn a_chosen_table_is_shown_as_a_table() {
    let file = jwst_like();
    let choices = Choices::default();
    let plugin = FitsPlugin::default().with_choices(choices.clone());
    choices.set(&file, 4usize);
    let img = plugin.decode(&file).unwrap();
    let table = img.table.expect("a table");
    assert_eq!((table.rows(), table.columns().len()), (1, 1));
    assert_eq!(img.metadata.get(GROUP, HDU), Some("4 ASDF"));
    assert_eq!(
        img.metadata.get(GROUP, TABLE),
        Some("BINTABLE 1 column x 1 row")
    );
    assert_eq!(plugin.frame_count(&file).unwrap(), 1);
}

#[test]
fn files_with_several_images_list_them_in_the_metadata() {
    let img = decode(&jwst_like());
    assert_eq!(img.metadata.get(GROUP, HDU), Some("1 SCI"));
    assert_eq!(img.metadata.get(PARTS_GROUP, "2 ERR"), Some("2 x 1  -32"));
    assert_eq!(img.metadata.get(PARTS_GROUP, "3 DQ"), Some("2 x 1  16"));
}

#[test]
fn a_file_with_one_image_has_no_hdu_list() {
    let img = decode(&fits(&image_cards(8, &[2, 1]), &[1, 2]));
    assert_eq!(img.metadata.get(GROUP, HDU), None);
    assert!(
        img.metadata
            .entries
            .iter()
            .all(|(g, _, _)| g != PARTS_GROUP)
    );
}

#[test]
fn hdus_of_the_same_name_carry_their_version() {
    let mut file = fits(&[card("BITPIX", "8"), card("NAXIS", "0")], &[]);
    for version in 1..=2 {
        let mut ext = vec![card("XTENSION", "'IMAGE   '")];
        ext.extend(image_cards(8, &[1, 1]));
        ext.push(card("EXTNAME", "'SCI'"));
        ext.push(card("EXTVER", &version.to_string()));
        file.extend(hdu_bytes(&ext, &[], &[version as u8]));
    }
    let names: Vec<String> = parts(&file).unwrap().into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["1 SCI", "2 SCI,2"]);
}

#[test]
fn file_without_an_image_or_a_table_is_unsupported() {
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
    file[at..at + 10].copy_from_slice(b"'LZ4_1   '");
    assert!(matches!(
        FitsPlugin::default().decode(&file),
        Err(DecodeError::Unsupported(m)) if m.contains("LZ4_1")
    ));
}

fn assert_frame_eq(compressed: &str, plain: &str, frame: usize) {
    let p = FitsPlugin::default();
    let a = f32s(&p.decode(&fixture(compressed)).unwrap());
    let b = f32s(&p.decode_frame(&fixture(plain), frame).unwrap());
    assert_eq!(a.len(), b.len(), "{compressed}");
    for (i, (x, y)) in a.iter().zip(&b).enumerate() {
        assert!(
            x == y || (x.is_nan() && y.is_nan()),
            "{compressed} at {i}: {x} vs {y}"
        );
    }
}

#[test]
fn lossless_hcompress_tiles_decode_to_the_original_pixels() {
    // tile は 10 x 6 で、右と下の端の tile は 5 x 5（2 の累乗でない大きさ）。
    assert_frame_eq("uint16-hcompress.fz", "uint16-cube.fits", 0);
    same_as("uint8-hcompress.fz", "uint8.fits");
    same_as("int32-hcompress.fz", "int32-shifted.fits");
}

#[test]
fn lossy_hcompress_matches_cfitsio_with_and_without_smoothing() {
    same_as(
        "int16-hcompress-scale4-smooth0.fz",
        "int16-hcompress-scale4-smooth0-expected.fits",
    );
    same_as(
        "int16-hcompress-scale4-smooth1.fz",
        "int16-hcompress-scale4-smooth1-expected.fits",
    );
}

#[test]
fn quantized_floats_in_hcompress_match_cfitsio() {
    same_as("float32-hcompress.fz", "float32-hcompress-expected.fits");
}

#[test]
fn plio_mask_decodes_to_the_original_values() {
    same_as("mask-plio.fz", "mask.fits");
}

#[test]
fn damaged_hcompress_and_plio_tiles_are_errors_not_crashes() {
    let p = FitsPlugin::default();
    let mut seed = 99u32;
    for name in [
        "uint16-hcompress.fz",
        "int16-hcompress-scale4-smooth1.fz",
        "mask-plio.fz",
    ] {
        let original = fixture(name);
        // 見出し（先頭の 2 ブロック）の後ろ、表と圧縮した中身のある所だけを壊す。
        let body = 2880 * 2;
        for _ in 0..300 {
            let mut file = original.clone();
            for _ in 0..3 {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let at = body + seed as usize % (file.len() - body);
                file[at] = (seed >> 16) as u8;
            }
            let _ = p.decode(&file);
        }
    }
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
fn headers_of_every_image_hdu_are_read_without_the_pixels() {
    let mut file = jwst_like();
    // Primary（ヘッダ 1 ブロック + データ 2 ブロック）を除いた、WCS 付きの画像の拡張。
    file.extend(file_with_wcs_after_data()[3 * 2880..].to_vec());
    let path = std::env::temp_dir().join(format!("vw-fits-{}-parts.fits", std::process::id()));
    std::fs::write(&path, &file).unwrap();
    let headers = image_headers_of_file(&path);
    std::fs::remove_file(&path).unwrap();
    let headers = headers.unwrap();
    let names: Vec<Option<&str>> = headers.iter().map(|h| h.get("EXTNAME")).collect();
    assert_eq!(names, [Some("SCI"), Some("ERR"), Some("DQ"), None]);
    assert!(headers[3].sky().is_some_and(|s| s.certain));
}

#[test]
fn decoded_images_carry_the_sky_of_each_pixel() {
    let file = file_with_wcs_after_data();
    let at = |flip_rows: bool, x: f64, y: f64| {
        let options = Options {
            flip_rows,
            ..Options::default()
        };
        decode_opts(&file, options).sky.unwrap().at(x, y).unwrap()
    };
    // 基準の画素 (CRPIX 2.5, 1.5) は、0 始まりの (1.5, 0.5)。上下を返しても同じ行（高さ 2 の真ん中）。
    for flip in [false, true] {
        let c = at(flip, 1.5, 0.5);
        assert!((c[0] - 83.8221).abs() < 1e-9 && (c[1] + 5.3911).abs() < 1e-9);
    }
    // 表示の一番上の行は、返さなければ FITS の y = 1、返せば y = 2（赤緯が 0.1° 北）。
    let north = at(true, 0.0, 0.0)[1] - at(false, 0.0, 0.0)[1];
    assert!((north - 0.1).abs() < 1e-3, "{north}");
}

#[test]
fn images_without_a_wcs_have_no_sky() {
    let img = decode(&fits(&image_cards(8, &[2, 1]), &[1, 2]));
    assert!(img.sky.is_none());
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
    assert_eq!(s.axis.value(1), 110e9);
    assert_eq!(s.axis.value(0), 110.5e9);
    assert_eq!(s.axis.velocity(110e9), Some(0.0));
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
    assert_eq!((v.kind.clone(), v.value(0)), (AxisKind::Velocity, 3000.0));
    let w = axis("WAVE", "nm");
    assert_eq!(w.kind, AxisKind::Wavelength);
    assert!((w.value(0) - 3e-9).abs() < 1e-20);
    let other = axis("PIXEL", "");
    assert!(matches!(other.kind, AxisKind::Other { .. }));
}

#[test]
fn wavelength_units_are_read_the_same_on_image_axes_and_table_columns() {
    // ヘッダは ASCII なので、µm はここでは試さない。
    for unit in ["um", "micron", "microns"] {
        let mut c = image_cards(-32, &[2]);
        c.extend([
            card("CTYPE1", "'WAVE    '"),
            card("CUNIT1", &format!("'{unit}'")),
            card("CRVAL1", "2.0"),
            card("CDELT1", "1.0"),
        ]);
        let axis = decode(&fits(&c, &f32_bytes(&[0.0, 1.0])))
            .spectrum
            .unwrap()
            .axis
            .clone();
        assert!(
            (axis.value(0) - 2e-6).abs() < 1e-18,
            "{unit}: {}",
            axis.value(0)
        );
        let tunit = format!("'{unit}'");
        let mut file = empty_primary();
        file.extend(table_hdu(
            "BINTABLE",
            8,
            2,
            &[
                ("WAVELENGTH", "1E", &[("TUNIT", tunit.as_str())]),
                ("FLUX", "1E", &[]),
            ],
            &[],
            &f32_bytes(&[2.0, 1.0, 3.0, 1.0]),
            0,
        ));
        let axis = decode(&file).spectrum.unwrap().axis.clone();
        assert!((axis.value(0) - 2e-6).abs() < 1e-18, "{unit} in a table");
    }
}

/// 256 点のスペクトルの FITS の、`cards` の軸の 1・38・200 画素目の値。
fn spectral_values(ctype: &str, cards: &[(&str, &str)]) -> (vw_core::spectrum::Axis, [f64; 3]) {
    let mut c = image_cards(-32, &[256]);
    c.push(card("CTYPE1", &format!("'{ctype:<8}'")));
    c.extend(cards.iter().map(|(k, v)| card(k, v)));
    let axis = decode(&fits(&c, &f32_bytes(&[0.0; 256])))
        .spectrum
        .expect("a spectrum")
        .axis
        .clone();
    let at = [0, 37, 199].map(|i| axis.value(i));
    (axis, at)
}

/// `CTYPE1`、ほかのカード、1・38・200 画素目の値。
type SpectralCase<'a> = (&'a str, &'a [(&'a str, &'a str)], [f64; 3]);

#[test]
fn nonlinear_spectral_axes_match_astropy() {
    // astropy 8.0.2（wcslib）の all_pix2world の値。
    let cases: [SpectralCase; 10] = [
        (
            "WAVE-F2W",
            &[
                ("CUNIT1", "'m'"),
                ("CRVAL1", "0.2110611405"),
                ("CDELT1", "1.5E-5"),
                ("CRPIX1", "32"),
            ],
            [2.105971627141e-01, 2.111511788939e-01, 2.136115920454e-01],
        ),
        (
            "VOPT-F2W",
            &[
                ("CUNIT1", "'m/s'"),
                ("CRVAL1", "1.0E6"),
                ("CDELT1", "5.0E3"),
                ("CRPIX1", "10"),
                ("RESTFRQ", "1.420405752E9"),
            ],
            [9.550067312096e+05, 1.140065191551e+06, 1.953009913960e+06],
        ),
        (
            "FREQ-W2F",
            &[
                ("CUNIT1", "'Hz'"),
                ("CRVAL1", "1.4E9"),
                ("CDELT1", "-1.0E5"),
                ("CRPIX1", "20"),
            ],
            [1.401902582076e+09, 1.398202311314e+09, 1.382228490832e+09],
        ),
        (
            "VELO-F2V",
            &[
                ("CUNIT1", "'m/s'"),
                ("CRVAL1", "-2.0E5"),
                ("CDELT1", "1.0E4"),
                ("CRPIX1", "5"),
                ("RESTFRQ", "1.420405752E9"),
            ],
            [-2.399973279261e+05, 1.301818682228e+05, 1.756350272021e+06],
        ),
        (
            "AWAV-W2A",
            &[
                ("CUNIT1", "'Angstrom'"),
                ("CRVAL1", "6562.8"),
                ("CDELT1", "0.5"),
                ("CRPIX1", "100"),
            ],
            [6.513299998486e-07, 6.531799999408e-07, 6.612799998480e-07],
        ),
        (
            "WAVE-A2W",
            &[
                ("CUNIT1", "'nm'"),
                ("CRVAL1", "656.28"),
                ("CDELT1", "0.05"),
                ("CRPIX1", "100"),
            ],
            [6.513300001515e-07, 6.531800000592e-07, 6.612800001521e-07],
        ),
        (
            "FREQ-V2F",
            &[
                ("CUNIT1", "'GHz'"),
                ("CRVAL1", "115.0"),
                ("CDELT1", "-0.002"),
                ("CRPIX1", "64"),
                ("RESTFRQ", "115.2712018E9"),
            ],
            [1.151260687762e+11, 1.150520117064e+11, 1.147283193983e+11],
        ),
        (
            "ZOPT-F2W",
            &[
                ("CRVAL1", "0.5"),
                ("CDELT1", "0.001"),
                ("CRPIX1", "1"),
                ("RESTWAV", "1215.67E-10"),
            ],
            [5.000000000000e-01, 5.379357484621e-01, 7.294388931591e-01],
        ),
        (
            "WAVE-LOG",
            &[
                ("CUNIT1", "'m'"),
                ("CRVAL1", "5.0E-7"),
                ("CDELT1", "1.0E-10"),
                ("CRPIX1", "1000"),
            ],
            [4.094472578022e-07, 4.124884058800e-07, 4.260718944831e-07],
        ),
        (
            "WAVE-GRI",
            &[
                ("CUNIT1", "'m'"),
                ("CRVAL1", "6.0E-7"),
                ("CDELT1", "2.0E-10"),
                ("CRPIX1", "512"),
                ("PV1_0", "1.0E5"),
                ("PV1_1", "1"),
                ("PV1_2", "20.0"),
                ("PV1_3", "1.5"),
                ("PV1_4", "1000.0"),
                ("PV1_6", "5.0"),
            ],
            [4.982047475943e-07, 5.055478900753e-07, 5.377500309465e-07],
        ),
    ];
    for (ctype, cards, expected) in cases {
        let (_, at) = spectral_values(ctype, cards);
        for (got, want) in at.iter().zip(expected) {
            assert!(
                ((got - want) / want).abs() < 1e-9,
                "{ctype}: {got:e} != {want:e}"
            );
        }
    }
}

#[test]
fn the_old_felo_axis_is_an_optical_velocity_linear_in_frequency() {
    let (axis, at) = spectral_values(
        "FELO-HEL",
        &[
            ("CUNIT1", "'m/s'"),
            ("CRVAL1", "1.5E6"),
            ("CDELT1", "-1.0E4"),
            ("CRPIX1", "8"),
            ("RESTFREQ", "1.420405752E9"),
        ],
    );
    for (got, want) in at
        .iter()
        .zip([1.570016267047e+06, 1.200298415950e+06, -4.078421881993e+05])
    {
        assert!(((got - want) / want).abs() < 1e-9, "{got:e} != {want:e}");
    }
    assert_eq!(axis.kind, vw_core::spectrum::AxisKind::Velocity);
    assert_eq!(axis.frame, "BARYCENT");
}

#[test]
fn a_wavelength_axis_with_a_rest_wavelength_gives_optical_velocity() {
    let (axis, _) = spectral_values(
        "WAVE",
        &[
            ("CUNIT1", "'Angstrom'"),
            ("CRVAL1", "6562.8"),
            ("CDELT1", "1.0"),
            ("CRPIX1", "1"),
            ("RESTWAV", "6562.8E-10"),
        ],
    );
    assert_eq!(axis.velocity(axis.value(0)), Some(0.0));
    let one_angstrom = axis.velocity(axis.value(1)).unwrap();
    assert!((one_angstrom - 45_681.0).abs() < 1.0, "{one_angstrom}");
}

#[test]
fn an_air_wavelength_axis_compares_with_the_rest_wavelength_in_air() {
    // 真空の 656.461 nm は、空気の中では約 656.280 nm。
    let (axis, _) = spectral_values(
        "AWAV",
        &[
            ("CUNIT1", "'Angstrom'"),
            ("CRVAL1", "6562.80"),
            ("CDELT1", "0.1"),
            ("CRPIX1", "1"),
            ("RESTWAV", "656.461E-9"),
        ],
    );
    let v = axis.velocity(axis.value(0)).unwrap();
    assert!(v.abs() < 1_000.0, "{v}");
}

#[test]
fn energy_axes_keep_the_unit_they_are_written_in() {
    let (axis, at) = spectral_values(
        "ENER",
        &[
            ("CUNIT1", "'keV'"),
            ("CRVAL1", "0.5"),
            ("CDELT1", "0.01"),
            ("CRPIX1", "1"),
        ],
    );
    assert_eq!(
        axis.kind,
        vw_core::spectrum::AxisKind::Other {
            name: "ENER".into(),
            unit: "keV".into()
        }
    );
    assert!((at[1] - 0.87).abs() < 1e-12, "{}", at[1]);
}

#[test]
fn a_velocity_axis_without_a_rest_frequency_is_not_converted() {
    let mut c = image_cards(-32, &[4]);
    c.extend([
        card("CTYPE1", "'VRAD-F2V'"),
        card("CRVAL1", "0.0"),
        card("CDELT1", "1.0"),
    ]);
    let s = decode(&fits(&c, &f32_bytes(&[0.0; 4]))).spectrum.unwrap();
    // 静止周波数がなければ周波数に直せないので、軸の種類だけを書いたものとして読まない。
    assert!(matches!(
        s.axis.kind,
        vw_core::spectrum::AxisKind::Other { .. }
    ));
}

#[test]
fn a_cube_plane_on_a_wavelength_axis_tells_its_optical_velocity_and_frame() {
    let file = cube(
        3,
        &[
            card("CTYPE3", "'WAVE-F2W'"),
            card("CRVAL3", "0.2110611405"),
            card("CRPIX3", "1"),
            card("CDELT3", "1.0E-5"),
            card("CUNIT3", "'m'"),
            card("RESTFRQ", "1.420405752E9"),
            card("SPECSYS", "'LSRK'"),
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
    assert_eq!(plane(0).as_deref(), Some("211.0611 mm, 0.00 km/s (LSRK)"));
    assert_eq!(plane(1).as_deref(), Some("211.0711 mm, 14.20 km/s (LSRK)"));
}

#[test]
fn a_written_image_reads_back_with_its_values_wcs_and_history() {
    let img = VwImage::new(
        2,
        2,
        ColorModel::GrayAlpha,
        PixelData::U16(vec![10, 65535, 4095, 65535, 7, 0, 1, 65535]),
    )
    .unwrap();
    let cards = vec![
        ("CTYPE1".to_owned(), "RA---TAN".to_owned()),
        ("CRPIX1".to_owned(), "1.5E0".to_owned()),
    ];
    let bytes = crate::encode(&img, &cards, &["made from a.fits and b.fits".to_owned()]).unwrap();
    assert_eq!(bytes.len() % crate::header::BLOCK, 0);
    let back = decode(&bytes);
    assert_eq!(
        (back.width, back.height, back.color),
        (2, 2, ColorModel::Gray)
    );
    let PixelData::F32(v) = &back.data else {
        panic!("not float: {:?}", back.color);
    };
    // 3 つ目の画素はアルファが 0 なので写っていない。
    assert_eq!(&v[..2], &[10.0, 4095.0]);
    assert!(v[2].is_nan());
    assert_eq!(v[3], 1.0);
    let lookup = |key: &str| {
        back.metadata
            .entries
            .iter()
            .find(|(_, k, _)| k == key)
            .map(|e| e.2.clone())
    };
    assert_eq!(
        lookup("CTYPE1")
            .as_deref()
            .map(|v| v.trim_matches('\'').trim()),
        Some("RA---TAN")
    );
    assert!(
        back.metadata
            .entries
            .iter()
            .any(|(_, _, v)| v.contains("made from a.fits")),
        "{:?}",
        back.metadata
    );
}

#[test]
fn a_sound_header_has_no_metadata_problems() {
    let mut cards = image_cards(8, &[2, 1]);
    cards.push(card("FILTER", "'Ha'"));
    let m = FitsPlugin::default()
        .read_metadata(&fits(&cards, &[9, 8]))
        .unwrap();
    assert_eq!(m.problems, []);
}

#[test]
fn a_card_with_bytes_outside_ascii_names_its_number_and_offset() {
    let mut cards = image_cards(8, &[2, 1]);
    cards.push(card("OBSERVER", "'Ana'"));
    let mut file = fits(&cards, &[9, 8]);
    let at = file.windows(3).position(|w| w == b"Ana").unwrap();
    file[at] = 0xC1;
    let n = cards.len() + 1; // SIMPLE の分
    let m = FitsPlugin::default().read_metadata(&file).unwrap();
    assert_eq!(m.problems.len(), 1, "{:?}", m.problems);
    assert_eq!(
        m.problems[0].place,
        format!("card {n} OBSERVER (offset {})", (n - 1) * 80)
    );
    assert!(m.problems[0].error.starts_with("1 byte(s) outside ASCII"));
}

#[test]
fn a_string_without_its_closing_quote_is_reported_and_still_shown() {
    let mut cards = image_cards(8, &[2, 1]);
    cards.push(card("OBJECT", "'M31"));
    let m = FitsPlugin::default()
        .read_metadata(&fits(&cards, &[9, 8]))
        .unwrap();
    assert_eq!(m.get(GROUP, "OBJECT"), Some("M31"));
    assert_eq!(m.problems.len(), 1);
    assert_eq!(m.problems[0].error, "string has no closing quote");
}

#[test]
fn the_spectrum_at_a_point_of_a_cube_runs_along_its_spectral_axis() {
    // 3 本目が偏光（長さ 2）、4 本目が周波数（長さ 3）。面の並びは偏光が速く回る。
    let mut cards = image_cards(-32, &[2, 2, 2, 3]);
    cards.extend([
        card("CTYPE3", "'STOKES'"),
        card("CRVAL3", "1"),
        card("CTYPE4", "'FREQ'"),
        card("CRVAL4", "1.4E9"),
        card("CRPIX4", "1"),
        card("CDELT4", "1.0E6"),
        card("CUNIT4", "'Hz'"),
        card("BUNIT", "'Jy/beam'"),
    ]);
    let values: Vec<f32> = (0..6)
        .flat_map(|plane| (0..4).map(move |i| (plane * 10 + i) as f32))
        .collect();
    let file = fits(&cards, &f32_bytes(&values));
    let mut cube = cube::Cube::new(&file, None, false).unwrap();
    // 面 3 = 偏光 2 本目・周波数 2 本目。画素 (1, 0) は 4 画素の 2 番目。
    let point = cube.spectrum(3, 1, 0).unwrap().unwrap();
    assert_eq!(&*point.spectrum.values, &[11.0, 31.0, 51.0]);
    assert_eq!(point.channel, 1);
    assert_eq!(point.spectrum.value_unit, "Jy/beam");
    assert_eq!(
        point.spectrum.axis.kind,
        vw_core::spectrum::AxisKind::Frequency
    );
    assert!((point.spectrum.axis.value(2) - 1.402e9).abs() < 1.0);
    assert_eq!(point.frame_of(2), 5);
}

#[test]
fn rows_turned_upside_down_are_read_where_they_are_shown() {
    let mut cards = image_cards(16, &[1, 2, 2]);
    cards.push(card("ROWORDER", "'BOTTOM-UP'"));
    let file = fits(&cards, &i16_bytes(&[1, 2, 3, 4]));
    let mut cube = cube::Cube::new(&file, None, false).unwrap();
    // 画面の上の行は、ファイルの 2 行目。
    assert_eq!(
        &*cube.spectrum(0, 0, 0).unwrap().unwrap().spectrum.values,
        &[2.0, 4.0]
    );
}

#[test]
fn the_spectrum_of_a_tile_compressed_cube_matches_its_pixels() {
    let file = fixture("int16-rice-cube.fz");
    let mut cube = cube::Cube::new(&file, None, false).unwrap();
    let point = cube.spectrum(0, 3, 2).unwrap().unwrap();
    assert_eq!(&*point.spectrum.values, &[23.0, 123.0, 223.0, 323.0, 423.0]);
    assert_eq!(
        point.spectrum.axis.kind,
        vw_core::spectrum::AxisKind::Velocity
    );
    // 2 点目は覚えた面から読む。
    let other = cube.spectrum(4, 0, 1).unwrap().unwrap();
    assert_eq!(&*other.spectrum.values, &[10.0, 110.0, 210.0, 310.0, 410.0]);
    assert_eq!(other.channel, 4);
}

#[test]
fn a_flat_image_has_no_spectrum_at_a_point() {
    let file = fits(&image_cards(16, &[2, 2]), &i16_bytes(&[1, 2, 3, 4]));
    assert!(
        cube::Cube::new(&file, None, false)
            .unwrap()
            .spectrum(0, 0, 0)
            .unwrap()
            .is_none()
    );
}

/// 2 x 1 画素、5 チャンネルの速度のキューブ（VRAD、-20 から 10 km/s ずつ）。左の画素は 0 km/s に
/// 高さ 4 の山（その両隣に 2）、右の画素は全部 0。
fn velocity_cube(extra: &[String]) -> Vec<u8> {
    velocity_cube_with(extra, [0.0; 5])
}

/// [`velocity_cube`] の、右の画素の値を `right` にしたもの。
fn velocity_cube_with(extra: &[String], right: [f32; 5]) -> Vec<u8> {
    let mut cards = image_cards(-32, &[2, 1, 5]);
    cards.extend([
        card("CTYPE1", "'RA---SIN'"),
        card("CRVAL1", "202.5"),
        card("CRPIX1", "1"),
        card("CDELT1", "-0.001"),
        card("CTYPE2", "'DEC--SIN'"),
        card("CRVAL2", "47.2"),
        card("CRPIX2", "1"),
        card("CDELT2", "0.001"),
        card("CTYPE3", "'VRAD'"),
        card("CRVAL3", "-20000"),
        card("CRPIX3", "1"),
        card("CDELT3", "10000"),
        card("CUNIT3", "'m/s'"),
        card("BUNIT", "'Jy/beam'"),
        card("BMAJ", "0.002"),
    ]);
    cards.extend(extra.iter().cloned());
    let left = [0.0f32, 2.0, 4.0, 2.0, 0.0];
    let values: Vec<f32> = left.iter().zip(right).flat_map(|(&l, r)| [l, r]).collect();
    fits(&cards, &f32_bytes(&values))
}

fn moment(file: &[u8], order: moments::Order, clip: f64) -> VwImage {
    let options = moments::Options {
        order,
        channels: None,
        clip,
        frame: 0,
        hdu: None,
    };
    decode(&moments::make(file, &options).unwrap())
}

#[test]
fn moment_zero_is_the_line_area_in_value_times_km_per_s() {
    let img = moment(&velocity_cube(&[]), moments::Order::Zero, 0.0);
    // (2 + 4 + 2) × 10 km/s。
    assert_eq!(f32s(&img), vec![80.0, 0.0]);
    assert_eq!(img.metadata.get(GROUP, "BUNIT"), Some("Jy/beam.km/s"));
}

#[test]
fn moment_one_is_the_weighted_velocity_and_two_its_spread() {
    let file = velocity_cube(&[]);
    let one = f32s(&moment(&file, moments::Order::One, 0.0));
    assert!(one[0].abs() < 1e-4, "{one:?}");
    assert!(one[1].is_nan(), "no signal, no velocity");
    let two = f32s(&moment(&file, moments::Order::Two, 0.0));
    // 重み 2・4・2 で -10・0・10 km/s: 分散は (2·100 + 2·100) / 8 = 50。
    assert!((two[0] - 50f32.sqrt()).abs() < 1e-4, "{two:?}");
    assert_eq!(
        moment(&file, moments::Order::One, 0.0)
            .metadata
            .get(GROUP, "BUNIT"),
        Some("km/s")
    );
}

#[test]
fn values_under_the_threshold_are_left_out() {
    // 全部の値の中央値は 1、雑音は 1.4826。1 倍のしきい値 2.48 を超えるのは左の山の頂上の 4 だけ。
    let file = velocity_cube_with(&[], [1.0, -1.0, 1.0, -1.0, 1.0]);
    let img = moment(&file, moments::Order::Zero, 1.0);
    assert_eq!(f32s(&img), vec![40.0, 0.0]);
    let all = moment(&file, moments::Order::Zero, 0.0);
    assert_eq!(f32s(&all), vec![80.0, 10.0]);
}

#[test]
fn a_chosen_channel_range_sums_only_those_channels() {
    let options = moments::Options {
        order: moments::Order::Zero,
        channels: Some((2, 3)),
        clip: 0.0,
        frame: 0,
        hdu: None,
    };
    let img = decode(&moments::make(&velocity_cube(&[]), &options).unwrap());
    assert_eq!(f32s(&img), vec![60.0, 0.0]);
    let outside = moments::Options {
        channels: Some((3, 9)),
        ..options
    };
    assert!(moments::make(&velocity_cube(&[]), &outside).is_err());
}

#[test]
fn a_moment_map_keeps_the_sky_coordinates_beam_and_row_order() {
    let file = velocity_cube(&[card("ROWORDER", "'BOTTOM-UP'")]);
    let img = moment(&file, moments::Order::Zero, 0.0);
    assert!(img.sky.is_some());
    assert_eq!(img.metadata.get(GROUP, "BMAJ"), Some("0.002"));
    assert_eq!(img.metadata.get(GROUP, "ROWORDER"), Some("BOTTOM-UP"));
    assert_eq!(img.metadata.get(GROUP, "NAXIS"), Some("2"));
}

#[test]
fn a_frequency_cube_with_a_rest_frequency_gives_velocities() {
    let mut cards = image_cards(-32, &[1, 1, 3]);
    cards.extend([
        card("CTYPE3", "'FREQ'"),
        card("CRVAL3", "1.42E9"),
        card("CRPIX3", "2"),
        card("CDELT3", "-1.0E5"),
        card("RESTFRQ", "1.42E9"),
    ]);
    let file = fits(&cards, &f32_bytes(&[0.0, 0.0, 1.0]));
    let one = f32s(&moment(&file, moments::Order::One, 0.0));
    // 3 チャンネル目は 1.4199 GHz: 電波の定義で c × 1e5 / 1.42e9 ≈ 21.1 km/s。
    let expected = 299_792.458 * 1.0e5 / 1.42e9;
    assert!((f64::from(one[0]) - expected).abs() < 1e-3, "{one:?}");
}

#[test]
fn the_channels_of_a_cube_are_read_from_its_header_alone() {
    let dir = std::env::temp_dir().join(format!("vw-fits-moments-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cube.fits");
    std::fs::write(&path, velocity_cube(&[])).unwrap();
    let found = moments::channels_of_file(&path, None).unwrap();
    assert_eq!(found.count, 5);
    assert!((found.axis.value(4) - 20_000.0).abs() < 1e-6);
    let flat = dir.join("flat.fits");
    std::fs::write(
        &flat,
        fits(&image_cards(16, &[2, 2]), &i16_bytes(&[1, 2, 3, 4])),
    )
    .unwrap();
    assert!(moments::channels_of_file(&flat, None).is_none());
}

/// 表の列 1 つ。`(TTYPE, TFORM, ほかのカード（鍵の頭, 値）)`。
type TestColumn<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);

/// 表の HDU。`data` は行のバイト列を並べ、その後ろにヒープ（`heap` バイト）を付けたもの。
fn table_hdu(
    xtension: &str,
    row_len: usize,
    rows: usize,
    columns: &[TestColumn],
    extra: &[String],
    data: &[u8],
    heap: usize,
) -> Vec<u8> {
    let mut cards = vec![
        card("XTENSION", &format!("'{xtension}'")),
        card("BITPIX", "8"),
        card("NAXIS", "2"),
        card("NAXIS1", &row_len.to_string()),
        card("NAXIS2", &rows.to_string()),
        card("PCOUNT", &heap.to_string()),
        card("GCOUNT", "1"),
        card("TFIELDS", &columns.len().to_string()),
    ];
    for (i, (name, form, more)) in columns.iter().enumerate() {
        let n = i + 1;
        cards.push(card(&format!("TTYPE{n}"), &format!("'{name}'")));
        cards.push(card(&format!("TFORM{n}"), &format!("'{form}'")));
        for (k, v) in more.iter() {
            cards.push(card(&format!("{k}{n}"), v));
        }
    }
    cards.extend_from_slice(extra);
    hdu_bytes(&cards, &[], data)
}

fn empty_primary() -> Vec<u8> {
    fits(&[card("BITPIX", "8"), card("NAXIS", "0")], &[])
}

fn table_of(file: &[u8]) -> Arc<dyn vw_core::table::Table> {
    FitsPlugin::default()
        .decode(file)
        .unwrap()
        .table
        .expect("a table")
}

fn row_cells(table: &dyn vw_core::table::Table, row: usize) -> Vec<String> {
    (0..table.columns().len())
        .map(|c| table.cell(row, c))
        .collect()
}

#[test]
fn binary_table_cells_follow_their_types_scales_and_nulls() {
    // 1J（TNULL -1）、1I（TSCAL 0.5、TZERO 10）、1I（符号なし、TZERO 32768）、6A、1L、3E、1D、1PE（ヒープ）、4X
    let row_len = 4 + 2 + 2 + 6 + 1 + 12 + 8 + 8 + 1;
    let mut data = Vec::new();
    let mut heap = Vec::new();
    for (j, i, u, a, l, arr, d, var, x) in [
        (
            7i32,
            4i16,
            -32768i16,
            *b"M31\0\0\0",
            b'T',
            [1.5f32, 2.25, 3.0],
            0.1f64,
            vec![9.5f32, 8.5],
            0b1010_0000u8,
        ),
        (
            -1,
            -2,
            32767,
            *b"NGC 22",
            b'F',
            [f32::NAN, 0.0, -1.0],
            1e20,
            vec![],
            0b0101_0000,
        ),
    ] {
        data.extend(j.to_be_bytes());
        data.extend(i.to_be_bytes());
        data.extend(u.to_be_bytes());
        data.extend(a);
        data.push(l);
        for v in arr {
            data.extend(v.to_be_bytes());
        }
        data.extend(d.to_be_bytes());
        data.extend((var.len() as u32).to_be_bytes());
        data.extend((heap.len() as u32).to_be_bytes());
        for v in &var {
            heap.extend(v.to_be_bytes());
        }
        data.push(x);
    }
    let heap_len = heap.len();
    data.extend(heap);
    let mut file = empty_primary();
    file.extend(table_hdu(
        "BINTABLE",
        row_len,
        2,
        &[
            ("ID", "1J", &[("TNULL", "-1")]),
            (
                "MAG",
                "1I",
                &[("TSCAL", "0.5"), ("TZERO", "10"), ("TUNIT", "'mag'")],
            ),
            ("COUNT", "1I", &[("TZERO", "32768")]),
            ("NAME", "6A", &[]),
            ("GOOD", "1L", &[]),
            ("RGB", "3E", &[]),
            ("TIME", "1D", &[]),
            ("SERIES", "1PE(2)", &[]),
            ("BITS", "4X", &[]),
        ],
        &[],
        &data,
        heap_len,
    ));
    let table = table_of(&file);
    assert_eq!(table.columns()[1].unit, "mag");
    assert_eq!(table.columns()[3].format, "6A");
    assert_eq!(
        row_cells(&*table, 0),
        [
            "7",
            "12",
            "0",
            "M31",
            "T",
            "[3] 1.5, 2.25, 3",
            "0.1",
            "[2] 9.5, 8.5",
            "1010"
        ]
    );
    assert_eq!(
        row_cells(&*table, 1),
        [
            "",
            "9",
            "65535",
            "NGC 22",
            "F",
            "[3] NaN, 0, -1",
            "1e20",
            "[0] ",
            "0101"
        ]
    );
}

#[test]
fn long_array_cells_show_only_their_first_values() {
    let values: Vec<f32> = (1..=10).map(|v| v as f32).collect();
    let mut file = empty_primary();
    file.extend(table_hdu(
        "BINTABLE",
        40,
        1,
        &[("FLUX", "10E", &[])],
        &[],
        &f32_bytes(&values),
        0,
    ));
    assert_eq!(table_of(&file).cell(0, 0), "[10] 1, 2, 3, 4, …");
}

#[test]
fn ascii_table_cells_are_read_from_their_columns() {
    let rows = b"Vega   0.03  1.2E+01Sirius***** -1.5D+00";
    let mut file = empty_primary();
    file.extend(table_hdu(
        "TABLE   ",
        20,
        2,
        &[
            ("STAR", "A6", &[("TBCOL", "1")]),
            ("V", "F6.2", &[("TBCOL", "7"), ("TNULL", "'*****'")]),
            ("FLUX", "E8.1", &[("TBCOL", "13"), ("TSCAL", "2")]),
        ],
        &[],
        rows,
        0,
    ));
    let table = table_of(&file);
    assert_eq!(row_cells(&*table, 0), ["Vega", "0.03", "24"]);
    assert_eq!(row_cells(&*table, 1), ["Sirius", "", "-3"]);
    assert_eq!(table.summary(), "TABLE 3 columns x 2 rows");
}

#[test]
fn a_file_with_only_a_table_opens_the_table() {
    let mut file = empty_primary();
    file.extend(table_hdu(
        "BINTABLE",
        4,
        1,
        &[("X", "1J", &[])],
        &[],
        &7i32.to_be_bytes(),
        0,
    ));
    let img = FitsPlugin::default().decode(&file).unwrap();
    assert_eq!(img.table.unwrap().cell(0, 0), "7");
    assert_eq!(
        img.metadata.get(GROUP, TABLE),
        Some("BINTABLE 1 column x 1 row")
    );
    assert_eq!(img.metadata.get(GROUP, HDU), None, "only one thing to see");
}

#[test]
fn a_table_of_wavelengths_and_fluxes_is_also_a_spectrum() {
    let mut data = Vec::new();
    for (w, f) in [(6000.0f32, 1.0f32), (6001.0, 2.0), (6002.0, 3.0)] {
        data.extend(w.to_be_bytes());
        data.extend(f.to_be_bytes());
        data.extend(0.1f32.to_be_bytes());
    }
    let mut file = empty_primary();
    file.extend(table_hdu(
        "BINTABLE",
        12,
        3,
        &[
            ("WAVELENGTH", "1E", &[("TUNIT", "'Angstrom'")]),
            ("FLUX", "1E", &[("TUNIT", "'erg/s/cm2/A'")]),
            ("FLUX_ERR", "1E", &[]),
        ],
        &[],
        &data,
        0,
    ));
    let img = FitsPlugin::default().decode(&file).unwrap();
    let s = img.spectrum.expect("a spectrum");
    assert_eq!(s.axis.kind, vw_core::spectrum::AxisKind::Wavelength);
    assert!((s.axis.values[1] - 6001e-10).abs() < 1e-16);
    assert_eq!(&*s.values, [1.0, 2.0, 3.0]);
    assert_eq!(s.value_unit, "erg/s/cm2/A");
    assert!(img.table.is_some(), "the table stays the main view");
}

#[test]
fn a_one_row_table_of_arrays_is_a_spectrum_of_its_arrays() {
    let mut data = Vec::new();
    for v in [3.6f32, 3.7, 3.8] {
        data.extend(v.to_be_bytes());
    }
    data.extend(f32_bytes(&[5.0, 6.0, 7.0]));
    let mut file = empty_primary();
    file.extend(table_hdu(
        "BINTABLE",
        24,
        1,
        &[("LOGLAM", "3E", &[]), ("FLUX", "3E", &[])],
        &[],
        &data,
        0,
    ));
    let s = FitsPlugin::default()
        .decode(&file)
        .unwrap()
        .spectrum
        .unwrap();
    assert!((s.axis.values[0] / (10f64.powf(3.6f32 as f64) * 1e-10) - 1.0).abs() < 1e-9);
    assert_eq!(&*s.values, [5.0, 6.0, 7.0]);
}

#[test]
fn a_table_without_wavelengths_is_not_a_spectrum() {
    let mut file = empty_primary();
    file.extend(table_hdu(
        "BINTABLE",
        4,
        1,
        &[("FLUX", "1E", &[])],
        &[],
        &f32_bytes(&[1.0]),
        0,
    ));
    assert!(
        FitsPlugin::default()
            .decode(&file)
            .unwrap()
            .spectrum
            .is_none()
    );
}

/// 1 次元のスペクトル（5 点）の軸を `-TAB` で書いたファイル。`index` なら番号の列も付ける。
fn tab_spectrum(index: bool) -> Vec<u8> {
    let mut cards = image_cards(-32, &[5]);
    cards.extend([
        card("CTYPE1", "'WAVE-TAB'"),
        card("CUNIT1", "'nm'"),
        card("CRPIX1", "1"),
        card("CRVAL1", "1"),
        card("CDELT1", "1"),
        card("PS1_0", "'WCS-TAB'"),
        card("PS1_1", "'COORDS'"),
    ]);
    if index {
        cards.push(card("PS1_2", "'INDEX'"));
    }
    let mut file = fits(&cards, &f32_bytes(&[1.0, 2.0, 3.0, 4.0, 5.0]));
    // 波長は等間隔でない（500, 501, 503, 507, 515 nm）。
    let coords = [500.0f64, 501.0, 503.0, 507.0, 515.0];
    let mut data: Vec<u8> = coords.iter().flat_map(|v| v.to_be_bytes()).collect();
    let mut columns: Vec<TestColumn> = vec![("COORDS", "5D", &[("TDIM", "'(1,5)'")])];
    if index {
        // 番号の列は 0.5 刻み: 画素 1 → 番号 1、画素 2 → 番号 1.5（座標の 1 番目と 2 番目の間）…
        data.extend(
            [1.0f64, 3.0, 5.0, 7.0, 9.0]
                .iter()
                .flat_map(|v| v.to_be_bytes()),
        );
        columns.push(("INDEX", "5D", &[]));
    }
    let row_len = data.len();
    file.extend(table_hdu(
        "BINTABLE",
        row_len,
        1,
        &columns,
        &[card("EXTNAME", "'WCS-TAB'")],
        &data,
        0,
    ));
    file
}

#[test]
fn a_tab_axis_takes_its_values_from_the_table() {
    let s = decode(&tab_spectrum(false)).spectrum.expect("a spectrum");
    let nm: Vec<f64> = s
        .axis
        .values
        .iter()
        .map(|v| (v * 1e9 * 1e6).round() / 1e6)
        .collect();
    assert_eq!(nm, [500.0, 501.0, 503.0, 507.0, 515.0]);
}

#[test]
fn a_tab_axis_with_an_index_column_looks_up_the_index_first() {
    let s = decode(&tab_spectrum(true)).spectrum.expect("a spectrum");
    let nm: Vec<f64> = s
        .axis
        .values
        .iter()
        .map(|v| (v * 1e9 * 1e6).round() / 1e6)
        .collect();
    // 画素 p の値 ψ = p。番号の列 [1,3,5,7,9] の中で ψ の所 → 座標の列を引く。
    assert_eq!(nm, [500.0, 500.5, 501.0, 502.0, 503.0]);
}

/// 2 x 1 画素、4 面のキューブ。3 本目の軸（周波数）を `-TAB` で書く（等間隔でない）。
fn tab_cube() -> Vec<u8> {
    let mut cards = image_cards(-32, &[2, 1, 4]);
    cards.extend([
        card("CTYPE3", "'FREQ-TAB'"),
        card("CUNIT3", "'MHz'"),
        card("CRPIX3", "1"),
        card("CRVAL3", "1"),
        card("CDELT3", "1"),
        card("PS3_0", "'WCS-TAB'"),
        card("PS3_1", "'COORDS'"),
    ]);
    let planes: Vec<f32> = (0..8).map(|v| v as f32).collect();
    let mut file = fits(&cards, &f32_bytes(&planes));
    let coords: Vec<u8> = [1400.0f64, 1401.0, 1403.0, 1406.0]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .collect();
    file.extend(table_hdu(
        "BINTABLE",
        32,
        1,
        &[("COORDS", "4D", &[("TDIM", "'(1,4)'")])],
        &[card("EXTNAME", "'WCS-TAB'")],
        &coords,
        0,
    ));
    file
}

#[test]
fn the_spectrum_at_a_cross_follows_a_tab_axis() {
    let mut cube = cube::Cube::new(&tab_cube(), None, false).unwrap();
    let point = cube.spectrum(0, 1, 0).unwrap().expect("a cube");
    let mhz: Vec<f64> = point.spectrum.axis.values.iter().map(|v| v / 1e6).collect();
    assert_eq!(mhz, [1400.0, 1401.0, 1403.0, 1406.0]);
    assert_eq!(&*point.spectrum.values, [1.0, 3.0, 5.0, 7.0]);
}

#[test]
fn the_channels_of_a_tab_cube_file_come_from_its_table() {
    let path = std::env::temp_dir().join(format!("vw-fits-{}-tab-cube.fits", std::process::id()));
    std::fs::write(&path, tab_cube()).unwrap();
    let channels = moments::channels_of_file(&path, None).expect("a cube");
    std::fs::remove_file(&path).ok();
    assert_eq!(channels.count, 4);
    assert_eq!(channels.axis.values[3], 1406e6);
}

/// 検出器 1 つの拡張。`values` は `w` × `h`（最初の行が FITS の y = 1）。
fn detector(w: usize, h: usize, values: &[f32], cards: &[String]) -> Vec<u8> {
    let mut ext = vec![card("XTENSION", "'IMAGE   '")];
    ext.extend(image_cards(-32, &[w, h]));
    ext.extend(cards.iter().cloned());
    hdu_bytes(&ext, &[], &f32_bytes(values))
}

fn empty_primary_with(cards: &[String]) -> Vec<u8> {
    let mut c = vec![card("BITPIX", "8"), card("NAXIS", "0")];
    c.extend(cards.iter().cloned());
    fits(&c, &[])
}

fn sec(key: &str, section: &str) -> String {
    card(key, &format!("'{section}'"))
}

/// 3 × 2 画素の検出器 2 つ。1〜2 列目が画素、3 列目がオーバースキャン（左は 5、右は 7）。
fn two_amplifiers(left: &str, right: &str) -> Vec<u8> {
    let mut file = empty_primary_with(&[]);
    for (detsec, bias, base) in [(left, 5.0, 10.0), (right, 7.0, 20.0)] {
        let values = [base, base + 1.0, bias, base + 2.0, base + 3.0, bias];
        file.extend(detector(
            3,
            2,
            &values,
            &[
                sec("DETSEC", detsec),
                sec("DATASEC", "[1:2,1:2]"),
                sec("BIASSEC", "[3:3,1:2]"),
            ],
        ));
    }
    file
}

#[test]
fn detectors_are_put_in_place_by_detsec_after_subtracting_the_overscan() {
    let img = decode(&two_amplifiers("[1:2,1:2]", "[3:4,1:2]"));
    assert_eq!((img.width, img.height), (4, 2));
    assert_eq!(f32s(&img), [5.0, 6.0, 13.0, 14.0, 7.0, 8.0, 15.0, 16.0]);
    assert!(
        img.metadata
            .get(GROUP, MOSAIC)
            .unwrap()
            .starts_with("2 detectors")
    );
    assert_eq!(img.metadata.get(GROUP, HDU), Some(MOSAIC));
}

#[test]
fn a_reversed_detsec_turns_the_detector_around() {
    let img = decode(&two_amplifiers("[1:2,1:2]", "[4:3,2:1]"));
    assert_eq!(f32s(&img), [5.0, 6.0, 16.0, 15.0, 7.0, 8.0, 14.0, 13.0]);
}

#[test]
fn gaps_between_detectors_are_missing() {
    let img = decode(&two_amplifiers("[1:2,1:2]", "[5:6,1:2]"));
    assert_eq!((img.width, img.height), (6, 2));
    let v = f32s(&img);
    assert!(v[2].is_nan() && v[3].is_nan() && v[8].is_nan());
    assert_eq!(v[4], 13.0);
}

#[test]
fn files_that_cannot_be_assembled_show_the_first_detector() {
    // 重なる DETSEC。
    let img = decode(&two_amplifiers("[1:2,1:2]", "[2:3,1:2]"));
    assert_eq!((img.width, img.height), (3, 2));
    assert!(img.metadata.get(GROUP, MOSAIC).is_none());
    // DETSEC のない画像がある。
    let mut file = empty_primary_with(&[]);
    file.extend(detector(1, 1, &[1.0], &[sec("DETSEC", "[1:1,1:1]")]));
    file.extend(detector(1, 1, &[2.0], &[]));
    assert_eq!(f32s(&decode(&file)), [1.0]);
    assert!(parts(&file).unwrap().iter().all(|p| p.name != MOSAIC));
}

#[test]
fn the_mosaic_comes_first_in_the_list_and_single_detectors_can_still_be_chosen() {
    let file = two_amplifiers("[1:2,1:2]", "[3:4,1:2]");
    let names: Vec<String> = parts(&file).unwrap().into_iter().map(|p| p.name).collect();
    assert_eq!(names, [MOSAIC, "1", "2"]);
    let choices = Choices::default();
    let plugin = FitsPlugin::default().with_choices(choices.clone());
    choices.set(&file, 2usize);
    assert_eq!(plugin.decode(&file).unwrap().width, 3);
    choices.set(&file, mosaic::MOSAIC);
    assert_eq!(plugin.decode(&file).unwrap().width, 4);
}

#[test]
fn the_setting_off_opens_the_first_detector_but_the_mosaic_can_be_chosen() {
    let file = two_amplifiers("[1:2,1:2]", "[3:4,1:2]");
    let off = Options {
        mosaic: false,
        ..Options::default()
    };
    assert_eq!(decode_opts(&file, off).width, 3);
    let chosen = decode_with(&file, 0, off, None, None, Some(mosaic::MOSAIC)).unwrap();
    assert_eq!(chosen.width, 4);
}

#[test]
fn large_mosaics_are_reduced_by_averaging_2_by_2() {
    let file = two_amplifiers("[1:2,1:2]", "[3:4,1:2]");
    let hdus = header::hdus(&file).unwrap();
    let layout = mosaic::layout(&hdus).unwrap();
    let a = mosaic::assemble(&file, &hdus, &layout, 4).unwrap();
    assert_eq!((a.width, a.height, a.bin), (2, 1, 2));
    assert_eq!(a.values, [6.5, 14.5]);
}

#[test]
fn the_mosaic_sky_is_that_of_the_detector_under_the_pixel() {
    let wcs = |crval1: &str| {
        vec![
            card("CTYPE1", "'RA---TAN'"),
            card("CTYPE2", "'DEC--TAN'"),
            card("CRPIX1", "1.0"),
            card("CRPIX2", "1.0"),
            card("CRVAL1", crval1),
            card("CRVAL2", "10.0"),
            card("CDELT1", "-0.001"),
            card("CDELT2", "0.001"),
        ]
    };
    let mut file = empty_primary_with(&[]);
    for (detsec, crval1) in [("[1:2,1:2]", "150.0"), ("[4:3,1:2]", "151.0")] {
        let mut cards = wcs(crval1);
        cards.push(sec("DETSEC", detsec));
        file.extend(detector(2, 2, &[1.0, 2.0, 3.0, 4.0], &cards));
    }
    let flipped = Options {
        flip_rows: true,
        ..Options::default()
    };
    let turned = decode_opts(&file, flipped).sky.unwrap();
    let sky = decode(&file).sky.unwrap();
    assert_eq!(turned.at(3.0, 0.0), sky.at(3.0, 1.0));
    // 2 × 2 に縮めた画素は、まとめた 4 画素の真ん中。
    let hdus = header::hdus(&file).unwrap();
    let layout = mosaic::layout(&hdus).unwrap();
    let binned = mosaic::MosaicSky::read(&hdus, &layout, 2, 1, false).unwrap();
    assert_eq!(
        vw_core::sky::SkyMap::at(&binned, 0.0, 0.0),
        sky.at(0.5, 0.5)
    );
    // 左の検出器の左下、右の検出器の左下（DETSEC が逆向きなので全体の右端）。
    let left = sky.at(0.0, 0.0).unwrap();
    assert!((left[0] - 150.0).abs() < 1e-9 && (left[1] - 10.0).abs() < 1e-9);
    let right = sky.at(3.0, 0.0).unwrap();
    assert!((right[0] - 151.0).abs() < 1e-9, "{right:?}");
    // 2 列目（検出器の x = 2）は 1 列目より西（赤経が小さい）。
    assert!(sky.at(1.0, 0.0).unwrap()[0] < left[0]);
    assert!(sky.at(2.0, 0.0).unwrap()[0] < right[0]);
}

#[test]
fn a_mosaic_pixel_tells_its_detector_and_the_pixel_there() {
    let wcs = [
        card("CTYPE1", "'RA---TAN'"),
        card("CTYPE2", "'DEC--TAN'"),
        card("CRPIX1", "1.0"),
        card("CRPIX2", "1.0"),
        card("CRVAL1", "150.0"),
        card("CRVAL2", "10.0"),
        card("CDELT1", "-0.001"),
        card("CDELT2", "0.001"),
    ];
    let mut file = empty_primary_with(&[]);
    for (detsec, name) in [("[1:2,1:2]", "'CCD1    '"), ("[4:3,1:2]", "'CCD2    '")] {
        let mut cards = wcs.to_vec();
        cards.push(sec("DETSEC", detsec));
        cards.push(card("EXTNAME", name));
        file.extend(detector(2, 2, &[1.0; 4], &cards));
    }
    let sky = decode(&file).sky.unwrap();
    assert_eq!(sky.part(1.0, 1.0), Some(("CCD1".to_owned(), [1.0, 1.0])));
    // 右の検出器は DETSEC が逆向きなので、全体の右端がその検出器の 1 列目。
    assert_eq!(sky.part(3.0, 0.0), Some(("CCD2".to_owned(), [0.0, 0.0])));
}

#[test]
fn sections_are_read_in_either_direction() {
    let s = mosaic::Section::parse(" [2048:1, 1:4096] ").unwrap();
    assert_eq!(
        (s.x, s.y, s.width(), s.height()),
        ((2048, 1), (1, 4096), 2048, 4096)
    );
    assert!(mosaic::Section::parse("[0:10,1:2]").is_none());
    assert!(mosaic::Section::parse("1:10,1:2").is_none());
}
