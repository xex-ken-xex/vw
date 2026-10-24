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

/// 無圧縮、1 ページ 1 ストリップの、リトルエンディアンの TIFF を作る。
fn build(pages: &[Page]) -> Vec<u8> {
    let mut out = b"II*\0".to_vec();
    out.extend(0u32.to_le_bytes());
    let mut link = 4; // 次の IFD の位置を書き込む場所
    for p in pages {
        let strip = out.len() as u32;
        out.extend(&p.pixels);
        let mut tags: Vec<(u16, V)> = vec![
            (256, V::Long(vec![p.width])),
            (257, V::Long(vec![p.height])),
            (273, V::Long(vec![strip])),
            (278, V::Long(vec![p.height])),
            (279, V::Long(vec![p.pixels.len() as u32])),
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
fn decodes_lzw_compressed_rgb() {
    use tiff::encoder::{colortype, Compression, TiffEncoder};
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
