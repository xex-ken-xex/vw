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
