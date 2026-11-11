use super::*;

/// テスト用のタグの値。
enum V {
    Short(Vec<u16>),
    Long(Vec<u32>),
    Ascii(&'static str),
}

/// 1 ページ分。`tags` に StripOffsets / StripByteCounts / RowsPerStrip は要らない（足す）。
struct Page {
    width: u32,
    height: u32,
    tags: Vec<(u16, V)>,
    pixels: Vec<u8>,
}

fn page(width: u32, height: u32, tags: Vec<(u16, V)>, pixels: Vec<u8>) -> Page {
    Page {
        width,
        height,
        tags,
        pixels,
    }
}

/// `tags` の PlanarConfiguration が 2 なら、面の数（SamplesPerPixel）。そうでなければ 1。
fn plane_count(tags: &[(u16, V)]) -> u32 {
    let short = |tag: u16| {
        tags.iter().find_map(|(t, v)| match v {
            V::Short(s) if *t == tag => s.first().copied(),
            _ => None,
        })
    };
    match short(284) {
        Some(2) => u32::from(short(277).unwrap_or(1)),
        _ => 1,
    }
}

/// 無圧縮、1 ページ 1 ストリップ（面を分けて持つ形は 1 面 1 ストリップ）の、リトルエンディアンの TIFF を作る。
fn build(pages: &[Page]) -> Vec<u8> {
    let mut out = b"II*\0".to_vec();
    out.extend(0u32.to_le_bytes());
    let mut link = 4; // 次の IFD の位置を書き込む場所
    for p in pages {
        let strip = out.len() as u32;
        out.extend(&p.pixels);
        let planes = plane_count(&p.tags);
        let plane_len = p.pixels.len() as u32 / planes;
        let mut tags: Vec<(u16, V)> = vec![
            (256, V::Long(vec![p.width])),
            (257, V::Long(vec![p.height])),
            (
                273,
                V::Long((0..planes).map(|k| strip + k * plane_len).collect()),
            ),
            (278, V::Long(vec![p.height])),
            (279, V::Long(vec![plane_len; planes as usize])),
        ];
        for (tag, v) in &p.tags {
            let v = match v {
                V::Short(s) => V::Short(s.clone()),
                V::Long(l) => V::Long(l.clone()),
                V::Ascii(a) => V::Ascii(a),
            };
            tags.push((*tag, v));
        }
        if !tags.iter().any(|(t, _)| *t == 259) {
            tags.push((259, V::Short(vec![1])));
        }
        tags.sort_by_key(|(t, _)| *t);
        if out.len() % 2 == 1 {
            out.push(0);
        }
        let ifd = out.len() as u32;
        out[link..link + 4].copy_from_slice(&ifd.to_le_bytes());
        let extra_at = ifd as usize + 2 + tags.len() * 12 + 4;
        let mut extra: Vec<u8> = Vec::new();
        out.extend((tags.len() as u16).to_le_bytes());
        for (tag, v) in &tags {
            let (ty, count, bytes): (u16, usize, Vec<u8>) = match v {
                V::Short(s) => (3, s.len(), s.iter().flat_map(|x| x.to_le_bytes()).collect()),
                V::Long(l) => (4, l.len(), l.iter().flat_map(|x| x.to_le_bytes()).collect()),
                V::Ascii(a) => {
                    let mut b = a.as_bytes().to_vec();
                    b.push(0);
                    (2, b.len(), b)
                }
            };
            out.extend(tag.to_le_bytes());
            out.extend(ty.to_le_bytes());
            out.extend((count as u32).to_le_bytes());
            if bytes.len() <= 4 {
                let mut b = bytes;
                b.resize(4, 0);
                out.extend(b);
            } else {
                out.extend(((extra_at + extra.len()) as u32).to_le_bytes());
                extra.extend(bytes);
                if extra.len() % 2 == 1 {
                    extra.push(0);
                }
            }
        }
        link = out.len();
        out.extend(0u32.to_le_bytes());
        out.extend(extra);
    }
    out
}

/// グレー / RGB などの基本のタグ。
fn basic(bits: &[u16], photometric: u16) -> Vec<(u16, V)> {
    vec![
        (258, V::Short(bits.to_vec())),
        (262, V::Short(vec![photometric])),
        (277, V::Short(vec![bits.len() as u16])),
    ]
}

fn decode(tiff: &[u8]) -> VwImage {
    TiffPlugin.decode(tiff).unwrap()
}

#[test]
fn probe_leaves_tiff_headers_to_the_extension() {
    // カメラ RAW と同じ先頭なので、中身では受け持たない。
    assert!(!TiffPlugin.probe(b"II*\0\x08\0\0\0"));
    assert!(!TiffPlugin.probe(b"MM\0*\0\0\0\x08"));
    assert_eq!(TiffPlugin.extensions(), ["tif", "tiff"]);
}

#[test]
fn decodes_rgb8() {
    let tiff = build(&[page(2, 1, basic(&[8, 8, 8], 2), vec![1, 2, 3, 4, 5, 6])]);
    let img = decode(&tiff);
    assert_eq!((img.width, img.height, img.color), (2, 1, ColorModel::Rgb));
    assert!(matches!(&img.data, PixelData::U8(v) if v == &[1, 2, 3, 4, 5, 6]));
}

#[test]
fn decodes_gray16_keeping_16_bits() {
    let px: Vec<u8> = [1000u16, 65535]
        .iter()
        .flat_map(|x| x.to_le_bytes())
        .collect();
    let img = decode(&build(&[page(2, 1, basic(&[16], 1), px)]));
    assert_eq!(img.color, ColorModel::Gray);
    assert!(matches!(&img.data, PixelData::U16(v) if v == &[1000, 65535]));
}

#[test]
fn decodes_rgba16() {
    let px: Vec<u8> = [1u16, 2, 3, 4]
        .iter()
        .flat_map(|x| x.to_le_bytes())
        .collect();
    let mut tags = basic(&[16, 16, 16, 16], 2);
    tags.push((338, V::Short(vec![2]))); // ExtraSamples: 関連付けのないアルファ
    let img = decode(&build(&[page(1, 1, tags, px)]));
    assert_eq!(img.color, ColorModel::Rgba);
    assert!(matches!(&img.data, PixelData::U16(v) if v == &[1, 2, 3, 4]));
}

#[test]
fn bilevel_white_is_zero_shows_zero_bits_as_white() {
    // 1bit、幅 10 で 1 行 2 バイト。先頭の 1 ビットだけが 1（黒）。
    let tiff = build(&[page(10, 1, basic(&[1], 0), vec![0b1000_0000, 0])]);
    let img = decode(&tiff);
    assert_eq!(img.color, ColorModel::Gray);
    let PixelData::U8(v) = &img.data else {
        panic!("8bit expected")
    };
    assert_eq!(v.len(), 10);
    assert_eq!(v[0], 0);
    assert!(v[1..].iter().all(|&x| x == 255));
}

#[test]
fn four_bit_gray_is_stretched_to_eight_bits() {
    // 幅 3 で 1 行 2 バイト（最後の 4 ビットは詰め物）。
    let tiff = build(&[page(3, 2, basic(&[4], 1), vec![0x0F, 0x80, 0x5A, 0x00])]);
    let img = decode(&tiff);
    assert!(matches!(&img.data, PixelData::U8(v) if v == &[0, 255, 136, 85, 170, 0]));
}

#[test]
fn palette_is_expanded_to_rgb() {
    let mut map = vec![0u16; 3 * 256];
    map[1] = 0xFF00; // 1 番 = 赤
    map[256 + 2] = 0x8000; // 2 番 = 暗い緑
    let mut tags = basic(&[8], 3);
    tags.push((320, V::Short(map)));
    let img = decode(&build(&[page(3, 1, tags, vec![1, 2, 0])]));
    assert_eq!(img.color, ColorModel::Rgb);
    assert!(matches!(&img.data, PixelData::U8(v) if v == &[255, 0, 0, 0, 128, 0, 0, 0, 0]));
    assert_eq!(img.metadata.get("TIFF", "Photometric"), Some("Palette"));
}

#[test]
fn four_bit_palette_is_expanded_to_rgb() {
    let mut map = vec![0u16; 3 * 16];
    map[15] = 0xFFFF;
    map[16 + 15] = 0xFFFF;
    map[32 + 15] = 0xFFFF;
    let mut tags = basic(&[4], 3);
    tags.push((320, V::Short(map)));
    let img = decode(&build(&[page(2, 1, tags, vec![0xF0])]));
    assert!(matches!(&img.data, PixelData::U8(v) if v == &[255, 255, 255, 0, 0, 0]));
}

#[test]
fn cmyk_is_converted_to_rgb() {
    // シアン 100%、黒 0% と、黒 100%。
    let px = vec![255, 0, 0, 0, 0, 0, 0, 255];
    let img = decode(&build(&[page(2, 1, basic(&[8, 8, 8, 8], 5), px)]));
    assert_eq!(img.color, ColorModel::Rgb);
    assert!(matches!(&img.data, PixelData::U8(v) if v == &[0, 255, 255, 0, 0, 0]));
}

#[test]
fn ycbcr_is_converted_to_rgb() {
    // 灰色と、赤（JPEG の式で RGB(255, 0, 0) にあたる YCbCr）。
    let mut tags = basic(&[8, 8, 8], 6);
    tags.push((530, V::Short(vec![1, 1]))); // YCbCrSubSampling: 間引きなし
    let img = decode(&build(&[page(
        2,
        1,
        tags,
        vec![128, 128, 128, 76, 85, 255],
    )]));
    assert_eq!(img.color, ColorModel::Rgb);
    assert!(matches!(&img.data, PixelData::U8(v) if v == &[128, 128, 128, 254, 0, 0]));
}

#[test]
fn floating_point_gray_is_kept_as_real_numbers() {
    let px: Vec<u8> = [-1.5f32, 2.25]
        .iter()
        .flat_map(|x| x.to_le_bytes())
        .collect();
    let mut tags = basic(&[32], 1);
    tags.push((339, V::Short(vec![3]))); // SampleFormat: 浮動小数点
    let img = decode(&build(&[page(2, 1, tags, px)]));
    assert!(matches!(&img.data, PixelData::F32(v) if v == &[-1.5, 2.25]));
    assert_eq!(
        img.metadata.get("TIFF", "Sample format"),
        Some("Floating point")
    );
}

#[test]
fn signed_16bit_gray_becomes_real_numbers() {
    let px: Vec<u8> = [-300i16, 7].iter().flat_map(|x| x.to_le_bytes()).collect();
    let mut tags = basic(&[16], 1);
    tags.push((339, V::Short(vec![2]))); // SampleFormat: 符号付き整数
    let img = decode(&build(&[page(2, 1, tags, px)]));
    assert!(matches!(&img.data, PixelData::F32(v) if v == &[-300.0, 7.0]));
}

#[test]
fn pages_skip_reduced_resolution_images() {
    let reduced = {
        let mut t = basic(&[8], 1);
        t.push((254, V::Long(vec![1]))); // NewSubfileType: 縮小版
        t
    };
    let tiff = build(&[
        page(2, 1, basic(&[8], 1), vec![10, 11]),
        page(1, 1, reduced, vec![99]),
        page(2, 1, basic(&[8], 1), vec![20, 21]),
    ]);
    assert_eq!(TiffPlugin.frame_count(&tiff).unwrap(), 2);
    let second = TiffPlugin.decode_frame(&tiff, 1).unwrap();
    assert!(matches!(&second.data, PixelData::U8(v) if v == &[20, 21]));
    assert_eq!(second.metadata.get("TIFF", "Page"), Some("2 / 2"));
    assert!(TiffPlugin.decode_frame(&tiff, 2).is_err());
}

#[test]
fn palette_on_a_later_page_is_expanded() {
    let mut map = vec![0u16; 3 * 256];
    map[256 + 1] = 0xFF00;
    let mut tags = basic(&[8], 3);
    tags.push((320, V::Short(map)));
    let tiff = build(&[
        page(1, 1, basic(&[8], 1), vec![0]),
        page(1, 1, tags, vec![1]),
    ]);
    let img = TiffPlugin.decode_frame(&tiff, 1).unwrap();
    assert!(matches!(&img.data, PixelData::U8(v) if v == &[0, 255, 0]));
}

#[test]
fn orientation_tag_rotates_the_pixels() {
    let mut tags = basic(&[8], 1);
    tags.push((274, V::Short(vec![6]))); // 右に 90 度回して見る
    let img = decode(&build(&[page(2, 1, tags, vec![1, 2])]));
    assert_eq!((img.width, img.height), (1, 2));
    assert!(matches!(&img.data, PixelData::U8(v) if v == &[1, 2]));
}

#[test]
fn metadata_lists_the_layout_and_the_text_tags_but_not_strip_offsets() {
    let mut tags = basic(&[8], 1);
    tags.push((271, V::Ascii("VW Camera Co.")));
    tags.push((305, V::Ascii("vw test")));
    let tiff = build(&[page(2, 1, tags, vec![0, 0])]);
    let m = TiffPlugin.read_metadata(&tiff).unwrap();
    assert_eq!(m.get("TIFF", "Size"), Some("2 x 1"));
    assert_eq!(m.get("TIFF", "Compression"), Some("None"));
    assert_eq!(m.get("TIFF", "Photometric"), Some("Black is zero"));
    assert_eq!(m.get("TIFF", "Page"), None);
    assert_eq!(m.get("EXIF", "Make"), Some("VW Camera Co."));
    assert_eq!(m.get("EXIF", "Software"), Some("vw test"));
    assert_eq!(m.get("EXIF", "StripOffsets"), None);
}

#[test]
fn planar_rgb8_is_interleaved() {
    let mut tags = basic(&[8, 8, 8], 2);
    tags.push((284, V::Short(vec![2])));
    // R の面、G の面、B の面の順。
    let img = decode(&build(&[page(2, 1, tags, vec![1, 4, 2, 5, 3, 6])]));
    assert_eq!((img.width, img.height, img.color), (2, 1, ColorModel::Rgb));
    assert!(matches!(&img.data, PixelData::U8(v) if v == &[1, 2, 3, 4, 5, 6]));
}

#[test]
fn planar_rgba16_is_interleaved() {
    let px: Vec<u8> = [1u16, 5, 2, 6, 3, 7, 4, 8]
        .iter()
        .flat_map(|x| x.to_le_bytes())
        .collect();
    let mut tags = basic(&[16, 16, 16, 16], 2);
    tags.push((284, V::Short(vec![2])));
    tags.push((338, V::Short(vec![2])));
    let img = decode(&build(&[page(2, 1, tags, px)]));
    assert_eq!(img.color, ColorModel::Rgba);
    assert!(matches!(&img.data, PixelData::U16(v) if v == &[1, 2, 3, 4, 5, 6, 7, 8]));
}

/// 3×3 画素の RGB を、16×16 のタイル 1 枚ずつの 3 面に分けて持つ TIFF。画素 i の値は R=i、G=10+i、B=20+i。
fn tiled_planar_3x3() -> Vec<u8> {
    let tile = 16 * 16;
    let mut out = b"II*\0".to_vec();
    out.extend(0u32.to_le_bytes());
    let first_tile = out.len() as u32;
    for plane in 0..3u8 {
        let mut t = vec![0u8; tile];
        for i in 0..9u8 {
            t[usize::from(i / 3) * 16 + usize::from(i % 3)] = plane * 10 + i;
        }
        out.extend(t);
    }
    let offsets_at = out.len() as u32;
    for k in 0..3u32 {
        out.extend((first_tile + k * tile as u32).to_le_bytes());
    }
    let counts_at = out.len() as u32;
    for _ in 0..3 {
        out.extend((tile as u32).to_le_bytes());
    }
    let bits_at = out.len() as u32;
    for _ in 0..3 {
        out.extend(8u16.to_le_bytes());
    }
    let ifd = out.len() as u32;
    out[4..8].copy_from_slice(&ifd.to_le_bytes());
    let entries: [(u16, u16, u32, u32); 11] = [
        (256, 3, 1, 3),
        (257, 3, 1, 3),
        (258, 3, 3, bits_at),
        (259, 3, 1, 1),
        (262, 3, 1, 2),
        (277, 3, 1, 3),
        (284, 3, 1, 2),
        (322, 3, 1, 16),
        (323, 3, 1, 16),
        (324, 4, 3, offsets_at),
        (325, 4, 3, counts_at),
    ];
    out.extend((entries.len() as u16).to_le_bytes());
    for (tag, kind, count, value) in entries {
        out.extend(tag.to_le_bytes());
        out.extend(kind.to_le_bytes());
        out.extend(count.to_le_bytes());
        out.extend(value.to_le_bytes());
    }
    out.extend(0u32.to_le_bytes());
    out
}

#[test]
fn planar_tiles_taller_than_the_image_are_cropped_in_every_plane() {
    let img = decode(&tiled_planar_3x3());
    assert_eq!((img.width, img.height, img.color), (3, 3, ColorModel::Rgb));
    let expected: Vec<u8> = (0..9u8).flat_map(|i| [i, 10 + i, 20 + i]).collect();
    assert!(matches!(&img.data, PixelData::U8(v) if *v == expected));
}

#[test]
fn planar_gray_reads_the_single_plane() {
    let mut tags = basic(&[8], 1);
    tags.push((284, V::Short(vec![2])));
    let img = decode(&build(&[page(3, 1, tags, vec![7, 8, 9])]));
    assert!(matches!(&img.data, PixelData::U8(v) if v == &[7, 8, 9]));
}

#[test]
fn decodes_lzw_compressed_rgb() {
    use tiff::encoder::{Compression, TiffEncoder, colortype};
    let pixels: Vec<u8> = (0..4 * 3 * 3).map(|i| i as u8).collect();
    let mut out = Cursor::new(Vec::new());
    TiffEncoder::new(&mut out)
        .unwrap()
        .with_compression(Compression::Lzw)
        .write_image::<colortype::RGB8>(4, 3, &pixels)
        .unwrap();
    let img = decode(out.get_ref());
    assert!(matches!(&img.data, PixelData::U8(v) if *v == pixels));
    assert_eq!(img.metadata.get("TIFF", "Compression"), Some("LZW"));
}

#[test]
fn broken_files_are_errors() {
    assert!(TiffPlugin.decode(b"II*\0\xff\xff\xff\xff").is_err());
    assert!(TiffPlugin.decode(b"not a tiff").is_err());
}

#[test]
fn huge_dimensions_in_the_header_are_refused_before_allocating() {
    // 100000 x 100000 の 16bit RGB（60 GB）を名乗る、1 画素分しか中身のない TIFF。
    let mut p = page(100_000, 100_000, basic(&[16, 16, 16], 2), vec![0; 6]);
    p.tags.push((278, V::Long(vec![100_000])));
    let tiff = build(&[p]);
    let err = TiffPlugin.decode(&tiff).unwrap_err().to_string();
    assert!(err.contains("100000 x 100000"), "{err}");
    assert!(err.contains("image size limit"), "{err}");
}

/// 最初の IFD から `tag` のエントリを取り除く（リトルエンディアンの TIFF）。
/// 装置が必須のタグを省いて書いたファイルを真似る。
fn drop_tag(tiff: &[u8], tag: u16) -> Vec<u8> {
    let mut out = tiff.to_vec();
    let ifd = u32::from_le_bytes(out[4..8].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(out[ifd..ifd + 2].try_into().unwrap()) as usize;
    let entries = ifd + 2;
    let at = (0..count)
        .map(|i| entries + i * 12)
        .find(|&e| u16::from_le_bytes(out[e..e + 2].try_into().unwrap()) == tag)
        .expect("tag present");
    // エントリと次の IFD へのポインタを 12 バイト前へ詰める。
    out.copy_within(at + 12..entries + count * 12 + 4, at);
    out[ifd..ifd + 2].copy_from_slice(&((count - 1) as u16).to_le_bytes());
    out
}

/// StripByteCounts のない 2x2 の 16bit グレー。
fn gray16_without_strip_byte_counts() -> Vec<u8> {
    let pixels: Vec<u8> = [1000u16, 2000, 3000, 65535]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    drop_tag(&build(&[page(2, 2, basic(&[16], 1), pixels)]), 279)
}

/// 補修の設定は静的な値なので、ON で読むテストは直列に動かす。
static REPAIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn with_repair_on<R>(f: impl FnOnce() -> R) -> R {
    let _guard = REPAIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_repair_strip_byte_counts(true);
    let r = f();
    set_repair_strip_byte_counts(false);
    r
}

#[test]
fn without_the_setting_a_missing_strip_byte_counts_names_the_setting_to_turn_on() {
    let _guard = REPAIR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tiff = gray16_without_strip_byte_counts();
    assert!(tiff::decoder::Decoder::new(Cursor::new(&tiff)).is_err());
    let err = TiffPlugin.decode(&tiff).unwrap_err().to_string();
    assert!(err.contains("Read files with format errors"), "{err}");
    assert!(TiffPlugin.frame_count(&tiff).is_err());
}

#[test]
fn gray16_without_strip_byte_counts_is_read_as_uncompressed() {
    let tiff = gray16_without_strip_byte_counts();
    let img = with_repair_on(|| decode(&tiff));
    assert_eq!((img.width, img.height), (2, 2));
    assert!(matches!(&img.data, PixelData::U16(v) if *v == [1000, 2000, 3000, 65535]));
    with_repair_on(|| {
        assert_eq!(TiffPlugin.frame_count(&tiff).unwrap(), 1);
        assert!(TiffPlugin.read_metadata(&tiff).is_ok());
    });
}

#[test]
fn rows_per_strip_may_be_omitted_too() {
    // 幅 3・高さ 2 の RGB8。RowsPerStrip も省く（既定は全行）。
    let pixels: Vec<u8> = (0..3 * 2 * 3).map(|i| i as u8 * 10).collect();
    let mut tags = basic(&[8, 8, 8], 2);
    tags.push((284, V::Short(vec![1])));
    let built = build(&[page(3, 2, tags, pixels.clone())]);
    let tiff = drop_tag(&drop_tag(&built, 279), 278);
    let img = with_repair_on(|| decode(&tiff));
    assert_eq!(img.color, ColorModel::Rgb);
    assert!(matches!(&img.data, PixelData::U8(v) if *v == pixels));
}

#[test]
fn compressed_files_without_strip_byte_counts_stay_errors() {
    use tiff::encoder::{Compression, TiffEncoder, colortype};
    let pixels: Vec<u8> = (0..4 * 3 * 3).map(|i| i as u8).collect();
    let mut out = Cursor::new(Vec::new());
    TiffEncoder::new(&mut out)
        .unwrap()
        .with_compression(Compression::Lzw)
        .write_image::<colortype::RGB8>(4, 3, &pixels)
        .unwrap();
    let tiff = drop_tag(out.get_ref(), 279);
    let err = with_repair_on(|| TiffPlugin.decode(&tiff).unwrap_err().to_string());
    assert!(!err.contains("Read files with format errors"), "{err}");
}

#[test]
fn a_strip_running_past_the_end_of_the_file_is_not_repaired() {
    let mut p = page(2, 2, basic(&[16], 1), vec![0; 8]);
    p.tags.push((257, V::Long(vec![1000])));
    let mut tiff = build(&[p]);
    // build が足した高さ 2 を 1000 に上書きしたので、画素は 8 バイトしかないのに 4000 バイト要る。
    tiff = drop_tag(&tiff, 279);
    assert!(with_repair_on(|| TiffPlugin.decode(&tiff)).is_err());
}

/// testdata/make_odd_bits_fixtures.py と同じ式の値（画素ごとにサンプルを並べた順）。
fn formula(width: u32, height: u32, samples: u32, bits: u32) -> Vec<u32> {
    let mut out = Vec::new();
    for y in 0..height {
        for x in 0..width {
            for c in 0..samples {
                out.push((x * 37 + y * 101 + c * 13 + 5) % (1 << bits));
            }
        }
    }
    out
}

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name);
    std::fs::read(path).unwrap()
}

fn assert_u16(img: &VwImage, want: &[u32]) {
    let PixelData::U16(v) = &img.data else {
        panic!("not 16-bit: {:?}", img.data.bits_per_sample());
    };
    let want: Vec<u16> = want.iter().map(|&x| x as u16).collect();
    assert_eq!(v, &want);
}

#[test]
fn twelve_bit_gray_keeps_its_values_and_twelve_significant_bits() {
    let img = decode(&fixture("gray12_strips_le.tif"));
    assert_eq!(
        (img.width, img.height, img.color),
        (13, 7, ColorModel::Gray)
    );
    assert_eq!(img.significant_bits, 12);
    assert_u16(&img, &formula(13, 7, 1, 12));
}

#[test]
fn twelve_bit_gray_in_lzw_and_big_endian_is_read() {
    let img = decode(&fixture("gray12_lzw_be.tif"));
    assert_u16(&img, &formula(13, 7, 1, 12));
}

#[test]
fn ten_bit_gray_in_deflate_tiles_is_cropped_to_the_image() {
    let img = decode(&fixture("gray10_deflate_tiles.tif"));
    assert_eq!((img.width, img.height), (21, 18));
    assert_eq!(img.significant_bits, 10);
    assert_u16(&img, &formula(21, 18, 1, 10));
}

#[test]
fn fourteen_bit_gray_in_packbits_is_read() {
    let img = decode(&fixture("gray14_packbits.tif"));
    assert_eq!(img.significant_bits, 14);
    assert_u16(&img, &formula(9, 5, 1, 14));
}

#[test]
fn twenty_four_bit_gray_becomes_exact_real_numbers() {
    let img = decode(&fixture("gray24_deflate.tif"));
    let PixelData::F32(v) = &img.data else {
        panic!("not real numbers");
    };
    let want: Vec<f32> = formula(11, 4, 1, 24).iter().map(|&x| x as f32).collect();
    assert_eq!(v, &want);
}

#[test]
fn twelve_bit_white_is_zero_is_inverted_within_twelve_bits() {
    let img = decode(&fixture("gray12_white_is_zero.tif"));
    let want: Vec<u32> = formula(5, 3, 1, 12).iter().map(|&x| 4095 - x).collect();
    assert_u16(&img, &want);
}

#[test]
fn twelve_bit_rgb_in_lzw_is_read() {
    let img = decode(&fixture("rgb12_lzw.tif"));
    assert_eq!(img.color, ColorModel::Rgb);
    assert_eq!(img.significant_bits, 12);
    assert_u16(&img, &formula(7, 5, 3, 12));
}

#[test]
fn twelve_bit_rgb_in_separate_planes_is_interleaved() {
    let img = decode(&fixture("rgb12_planar_deflate.tif"));
    assert_eq!(img.color, ColorModel::Rgb);
    assert_u16(&img, &formula(7, 5, 3, 12));
}

#[test]
fn ten_bit_rgba_in_separate_plane_tiles_is_interleaved_and_cropped() {
    let img = decode(&fixture("rgba10_planar_tiles.tif"));
    assert_eq!(
        (img.width, img.height, img.color),
        (19, 17, ColorModel::Rgba)
    );
    assert_u16(&img, &with_alpha_stretched(formula(19, 17, 4, 10), 4, 10));
}

#[test]
fn fourteen_bit_rgba_in_big_endian_is_read() {
    let img = decode(&fixture("rgba14_be.tif"));
    assert_eq!(img.color, ColorModel::Rgba);
    assert_u16(&img, &with_alpha_stretched(formula(6, 3, 4, 14), 4, 14));
}

/// アルファ（各画素の最後のサンプル）だけを 16bit の全範囲に引き伸ばしたもの。表示は 16bit の
/// アルファの最大を不透明とみなすため。色の値はそのまま。
fn with_alpha_stretched(mut values: Vec<u32>, samples: usize, bits: u32) -> Vec<u32> {
    let max = (1u64 << bits) - 1;
    for px in values.chunks_mut(samples) {
        let a = &mut px[samples - 1];
        *a = (u64::from(*a) * 65535 / max) as u32;
    }
    values
}

/// 値を上位ビットから `bits` ビットずつ詰めた 1 行（末尾は 1 バイトに切り上げ）。
fn pack_bits(values: &[u32], bits: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut acc, mut have) = (0u64, 0u32);
    for &v in values {
        acc = (acc << bits) | u64::from(v & ((1 << bits) - 1));
        have += bits;
        while have >= 8 {
            have -= 8;
            out.push((acc >> have) as u8);
        }
    }
    if have > 0 {
        out.push((acc << (8 - have)) as u8);
    }
    out
}

#[test]
fn signed_twelve_bit_gray_becomes_real_numbers_with_its_sign() {
    let mut tags = basic(&[12], 1);
    tags.push((339, V::Short(vec![2])));
    let raw = [0xFFF, 0x000, 0x7FF, 0x800]; // -1, 0, 2047, -2048
    let img = decode(&build(&[page(4, 1, tags, pack_bits(&raw, 12))]));
    assert!(matches!(&img.data, PixelData::F32(v) if v == &[-1.0, 0.0, 2047.0, -2048.0]));
}

#[test]
fn odd_bits_with_a_predictor_are_refused_by_name() {
    let mut tags = basic(&[12], 1);
    tags.push((317, V::Short(vec![2])));
    let err = TiffPlugin
        .decode(&build(&[page(2, 1, tags, pack_bits(&[1, 2], 12))]))
        .unwrap_err();
    assert!(
        err.to_string().contains("12-bit samples and a predictor"),
        "{err}"
    );
}

#[test]
fn twelve_bit_gray_metadata_says_twelve_bits() {
    let img = decode(&fixture("gray12_strips_le.tif"));
    assert_eq!(img.metadata.get("TIFF", "Bits per sample"), Some("12"));
}

#[test]
fn written_images_read_back_with_their_values_and_description() {
    let gray16 =
        VwImage::new(3, 1, ColorModel::Gray, PixelData::U16(vec![0, 4095, 65535])).unwrap();
    let rgba8 = VwImage::new(1, 1, ColorModel::Rgba, PixelData::U8(vec![1, 2, 3, 0])).unwrap();
    let float = VwImage::new(2, 1, ColorModel::Gray, PixelData::F32(vec![-1.5, f32::NAN])).unwrap();
    let gray_alpha =
        VwImage::new(1, 1, ColorModel::GrayAlpha, PixelData::U8(vec![9, 255])).unwrap();
    for (img, color) in [
        (&gray16, ColorModel::Gray),
        (&rgba8, ColorModel::Rgba),
        (&float, ColorModel::Gray),
        (&gray_alpha, ColorModel::Rgba),
    ] {
        let bytes = crate::encode(img, "made by a test").unwrap();
        let back = TiffPlugin.decode(&bytes).unwrap();
        assert_eq!(
            (back.width, back.height, back.color),
            (img.width, img.height, color)
        );
        assert!(
            back.metadata
                .entries
                .iter()
                .any(|(_, _, v)| v == "made by a test"),
            "{:?}",
            back.metadata
        );
    }
    let back = TiffPlugin
        .decode(&crate::encode(&gray16, "").unwrap())
        .unwrap();
    assert!(matches!(&back.data, PixelData::U16(v) if v == &[0, 4095, 65535]));
    let back = TiffPlugin
        .decode(&crate::encode(&float, "").unwrap())
        .unwrap();
    let PixelData::F32(v) = back.data else {
        panic!("not float");
    };
    assert_eq!(v[0], -1.5);
    assert!(v[1].is_nan());
    let back = TiffPlugin
        .decode(&crate::encode(&gray_alpha, "").unwrap())
        .unwrap();
    assert!(matches!(&back.data, PixelData::U8(v) if v == &[9, 9, 9, 255]));
}

// --- tiff クレートが読まない形（testdata/make_more_fixtures.py で libtiff などが作ったもの）---

/// 4 色の絵（左上 赤、右上 緑、左下 青、右下 白）の各色の中の 1 画素が、`tolerance` 以内でその色か。
fn assert_quadrants(img: &VwImage, tolerance: u8, which: &[usize]) {
    const QUADS: [[u8; 3]; 4] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]];
    let (w, h) = (img.width as usize, img.height as usize);
    let points = [
        (1, 1),
        (w / 2 + 1, 1),
        (1, h / 2 + 1),
        (w / 2 + 1, h / 2 + 1),
    ];
    let channels = img.data.len() / (w * h);
    for &q in which {
        let (x, y) = points[q];
        let at = (y * w + x) * channels;
        let got: Vec<u8> = match &img.data {
            PixelData::U8(v) => v[at..at + 3].to_vec(),
            PixelData::U16(v) => v[at..at + 3].iter().map(|&c| (c / 257) as u8).collect(),
            PixelData::F32(_) => panic!("real numbers"),
        };
        for (g, want) in got.iter().zip(QUADS[q]) {
            assert!(
                g.abs_diff(want) <= tolerance,
                "quadrant {q}: {got:?} != {:?}",
                QUADS[q]
            );
        }
    }
}

const ALL_QUADRANTS: &[usize] = &[0, 1, 2, 3];

#[test]
fn fax_reference_pictures_agree_whichever_color_is_zero() {
    let white_is_zero = u8_of(&decode(&fixture("fax_none_white_is_zero.tif")));
    let black_is_zero = u8_of(&decode(&fixture("fax_none_black_is_zero.tif")));
    assert_eq!(white_is_zero, black_is_zero);
    // 絵の中に黒も白もある（全部同じ色では比べた意味がない）。
    assert!(black_is_zero.contains(&0) && black_is_zero.contains(&255));
}

#[test]
fn every_ccitt_fax_variant_decodes_to_the_same_picture() {
    let want = u8_of(&decode(&fixture("fax_none_black_is_zero.tif")));
    for name in [
        "fax_huffman.tif",
        "fax_huffman_word.tif",
        "fax_g3_1d.tif",
        "fax_g3_2d.tif",
        "fax_g3_2d_fill.tif",
        "fax_g3_2d_lsb.tif",
    ] {
        assert_eq!(u8_of(&decode(&fixture(name))), want, "{name}");
    }
}

fn u8_of(img: &VwImage) -> Vec<u8> {
    match &img.data {
        PixelData::U8(v) => v.clone(),
        _ => panic!("not 8-bit"),
    }
}

#[test]
fn fax_metadata_names_the_original_compression() {
    let img = decode(&fixture("fax_g3_2d.tif"));
    assert_eq!(
        img.metadata.get("TIFF", "Compression"),
        Some("CCITT Group 3")
    );
}

#[test]
fn a_truncated_fax_strip_is_an_error() {
    let mut tiff = fixture("fax_g3_2d.tif");
    // 1 つ目のストリップの中身を 0 で潰す（EOL も符号も読めない）。
    let start = 8;
    tiff[start..start + 20].fill(0);
    assert!(TiffPlugin.decode(&tiff).is_err());
}

#[test]
fn lzma_and_zstd_decode_to_the_uncompressed_pixels() {
    let want = u8_of(&decode(&fixture("rgb_none.tif")));
    for name in [
        "rgb_lzma.tif",
        "rgb_zstd.tif",
        "rgb_zstd_predictor_tiles.tif",
    ] {
        assert_eq!(u8_of(&decode(&fixture(name))), want, "{name}");
    }
}

#[test]
fn webp_compressed_tiff_shows_the_four_colors() {
    assert_quadrants(&decode(&fixture("rgb_webp.tif")), 8, ALL_QUADRANTS);
}

#[test]
fn every_lab_kind_is_converted_to_srgb() {
    // 8bit の L*a*b* は 1 段の差が大きく、緑の赤の値のように 0 に近い所は数段ずれる（ガンマの立ち上がりが急なため）。
    for (name, tolerance) in [
        ("lab_cielab8.tif", 12),
        ("lab_cielab16.tif", 2),
        ("lab_icclab8.tif", 12),
        ("lab_icclab16.tif", 2),
    ] {
        let img = decode(&fixture(name));
        assert_eq!(img.color, ColorModel::Rgb, "{name}");
        assert_quadrants(&img, tolerance, ALL_QUADRANTS);
    }
    // ITULab の既定の範囲に入らない緑と青は見ない。
    assert_quadrants(&decode(&fixture("lab_itulab8.tif")), 12, &[0, 3]);
}

#[test]
fn sixteen_bit_lab_stays_sixteen_bit() {
    let img = decode(&fixture("lab_cielab16.tif"));
    assert!(matches!(img.data, PixelData::U16(_)));
}

fn f32_at(img: &VwImage, x: usize, y: usize) -> Vec<f32> {
    let PixelData::F32(v) = &img.data else {
        panic!("not real numbers")
    };
    let channels = v.len() / (img.width * img.height) as usize;
    let at = (y * img.width as usize + x) * channels;
    v[at..at + channels].to_vec()
}

#[test]
fn log_l_keeps_the_luminance_as_real_numbers() {
    let img = decode(&fixture("logl.tif"));
    assert_eq!(img.color, ColorModel::Gray);
    for (i, want) in [0.5f32, 1.0, 100.0, 1000.0].into_iter().enumerate() {
        let got = f32_at(&img, i * 2, 3)[0];
        assert!((got / want - 1.0).abs() < 0.005, "{got} != {want}");
    }
}

#[test]
fn logluv_gray_stays_neutral_and_red_stays_red() {
    let img = decode(&fixture("logluv.tif"));
    assert_eq!(img.color, ColorModel::Rgb);
    let gray = f32_at(&img, 2, 2);
    for c in &gray {
        assert!((c / 50.0 - 1.0).abs() < 0.02, "{gray:?}");
    }
    let red = f32_at(&img, 8, 2);
    assert!(red[0] > 10.0 * red[1].max(red[2]), "{red:?}");
}

#[test]
fn subsampled_ycbcr_is_upsampled_to_rgb() {
    for name in [
        "ycbcr_2x2.tif",
        "ycbcr_4x2.tif",
        "ycbcr_2x1.tif",
        "ycbcr_2x2_odd_lzw.tif",
    ] {
        let img = decode(&fixture(name));
        assert_eq!(img.color, ColorModel::Rgb, "{name}");
        assert_quadrants(&img, 3, ALL_QUADRANTS);
    }
    let odd = decode(&fixture("ycbcr_2x2_odd_lzw.tif"));
    assert_eq!((odd.width, odd.height), (37, 23));
}

/// make_more_fixtures.py の多バンドの値: 画素 (x, y) の c 番目のバンドは (x * 37 + y * 101 + c * 1000 + 5) mod 65536。
fn band_values(width: u32, height: u32, band: u32) -> Vec<u32> {
    (0..height)
        .flat_map(|y| (0..width).map(move |x| (x * 37 + y * 101 + band * 1000 + 5) % 65536))
        .collect()
}

#[test]
fn each_band_of_a_multiband_page_is_a_frame() {
    for (name, bands) in [
        ("bands5.tif", 5),
        ("bands5_planar.tif", 5),
        ("bands6_rgb.tif", 6),
    ] {
        let tiff = fixture(name);
        assert_eq!(TiffPlugin.frame_count(&tiff).unwrap(), bands, "{name}");
        for band in 0..bands {
            let img = TiffPlugin.decode_frame(&tiff, band).unwrap();
            assert_eq!(img.color, ColorModel::Gray);
            assert_u16(&img, &band_values(9, 7, band as u32));
            let label = format!("{} / {bands}", band + 1);
            assert_eq!(img.metadata.get("TIFF", "Band"), Some(label.as_str()));
        }
    }
}

#[test]
fn gray_with_alpha_is_one_gray_frame() {
    let tiff = fixture("gray_alpha.tif");
    assert_eq!(TiffPlugin.frame_count(&tiff).unwrap(), 1);
    let img = decode(&tiff);
    assert_eq!(img.color, ColorModel::GrayAlpha);
    let PixelData::U8(v) = &img.data else {
        panic!()
    };
    assert_eq!(&v[..2], &[200, 100]);
}

#[test]
fn floating_point_cmyk_is_converted_to_rgb() {
    let img = decode(&fixture("cmyk_float.tif"));
    assert_eq!(img.color, ColorModel::Rgb);
    // シアン 100%・墨 50% → 赤 0、緑と青は半分。
    assert_eq!(f32_at(&img, 0, 0), [0.0, 0.5, 0.5]);
}

#[test]
fn damaged_files_of_the_new_kinds_are_errors_not_crashes() {
    let names = [
        "fax_huffman.tif",
        "fax_huffman_word.tif",
        "fax_g3_1d.tif",
        "fax_g3_2d.tif",
        "fax_g3_2d_lsb.tif",
        "rgb_lzma.tif",
        "rgb_zstd_predictor_tiles.tif",
        "rgb_webp.tif",
        "lab_cielab16.tif",
        "logluv.tif",
        "logl.tif",
        "ycbcr_4x2.tif",
        "ycbcr_2x2_odd_lzw.tif",
        "bands5_planar.tif",
    ];
    let mut seed = 12345u32;
    for name in names {
        let original = fixture(name);
        for _ in 0..200 {
            let mut tiff = original.clone();
            for _ in 0..4 {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let at = seed as usize % tiff.len();
                tiff[at] = (seed >> 16) as u8;
            }
            let _ = TiffPlugin.frame_count(&tiff);
            let _ = TiffPlugin.decode(&tiff);
        }
    }
}

#[test]
fn a_sound_file_has_no_metadata_problems() {
    let mut tags = basic(&[8], 1);
    tags.push((271, V::Ascii("VW Camera Co.")));
    let tiff = build(&[page(2, 1, tags, vec![0, 0])]);
    let m = TiffPlugin.read_metadata(&tiff).unwrap();
    assert_eq!(m.problems, []);
}

#[test]
fn a_tag_of_the_wrong_type_is_reported_with_its_number() {
    let mut tags = basic(&[8], 1);
    tags.push((700, V::Long(vec![70_000, 80_000])));
    let tiff = build(&[page(2, 1, tags, vec![0, 0])]);
    let m = TiffPlugin.read_metadata(&tiff).unwrap();
    assert_eq!(m.problems.len(), 1, "{:?}", m.problems);
    assert_eq!(m.problems[0].place, "(0x02BC) XMP");
}
