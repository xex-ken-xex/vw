use super::*;
use vw_core::exif::{push_exif, sample, Gps};
use vw_core::Metadata;

const XMP: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF><rdf:Description xmp:Rating="3" exif:GPSLatitude="43,3.7N" exif:GPSLongitude = '141,21.2E'><photoshop:City>Sapporo</photoshop:City><Iptc4xmpCore:Location/><dc:subject><rdf:Bag><rdf:li>trip</rdf:li></rdf:Bag></dc:subject></rdf:Description></rdf:RDF></x:xmpmeta>"#;

fn segment(marker: u8, body: &[u8]) -> Vec<u8> {
    let mut s = vec![0xFF, marker];
    s.extend(((body.len() + 2) as u16).to_be_bytes());
    s.extend(body);
    s
}

fn jpeg(exif: &[u8]) -> Vec<u8> {
    let mut j = vec![0xFF, 0xD8];
    j.extend(segment(0xE1, &[b"Exif\0\0".as_slice(), exif].concat()));
    j.extend(segment(
        0xE1,
        &[b"http://ns.adobe.com/xap/1.0/\0".as_slice(), XMP.as_bytes()].concat(),
    ));
    j.extend([0xFF, 0xD9]);
    j
}

fn location(tiff: &[u8]) -> Option<Gps> {
    let mut m = Metadata::default();
    push_exif(&mut m, tiff.to_vec());
    Gps::from_metadata(&m)
}

#[test]
fn jpeg_loses_the_gps_and_the_xmp_places_but_keeps_its_length_and_the_rest() {
    let exif = sample::gps(43.0621, 141.3544, Some(20.0));
    assert!(location(&exif).is_some());
    let mut bytes = jpeg(&exif);
    let before = bytes.len();
    let removed = strip(&mut bytes).unwrap();
    assert!(removed.gps);
    assert_eq!(removed.xmp, 4);
    assert_eq!(bytes.len(), before);
    assert_eq!(location(&bytes[12..12 + exif.len()]), None);
    let text = String::from_utf8_lossy(&bytes);
    for gone in ["GPS", "Sapporo", "Iptc4xmpCore:Location", "43,3.7N"] {
        assert!(!text.contains(gone), "{gone} is left");
    }
    for kept in [
        "xmp:Rating=\"3\"",
        "<rdf:li>trip</rdf:li>",
        "</rdf:Description>",
    ] {
        assert!(text.contains(kept), "{kept} is gone");
    }
}

#[test]
fn the_other_tags_of_the_first_ifd_stay_readable() {
    let mut exif = sample::make(b"VWC");
    // Make の後ろに GPS の指し先を足した IFD0 を作る（項目 2 つ）。
    exif[8..10].copy_from_slice(&2u16.to_le_bytes());
    let gps_entry: Vec<u8> = [
        0x8825u16.to_le_bytes().as_slice(),
        &4u16.to_le_bytes(),
        &1u32.to_le_bytes(),
        &0u32.to_le_bytes(),
    ]
    .concat();
    exif.splice(22..22, gps_entry);
    let mut bytes = jpeg(&exif);
    assert!(strip(&mut bytes).unwrap().gps);
    let mut m = Metadata::default();
    push_exif(&mut m, bytes[12..12 + exif.len()].to_vec());
    assert_eq!(m.get("EXIF", "Make"), Some("VWC"));
    assert_eq!(m.get("EXIF", "GPSInfo"), None);
}

#[test]
fn png_exif_chunk_is_stripped_and_its_checksum_rewritten() {
    let mut png = Vec::new();
    {
        let mut e = png::Encoder::new(&mut png, 1, 1);
        e.set_color(png::ColorType::Grayscale);
        e.write_header().unwrap().write_image_data(&[7]).unwrap();
    }
    let exif = sample::gps(35.0, 139.0, None);
    let mut chunk = (exif.len() as u32).to_be_bytes().to_vec();
    chunk.extend(b"eXIf");
    chunk.extend(&exif);
    chunk.extend(crc32(&chunk[4..]).to_be_bytes());
    let iend = png.len() - 12;
    png.splice(iend..iend, chunk);
    assert!(strip(&mut png).unwrap().gps);
    let mut reader = png::Decoder::new(std::io::Cursor::new(&png))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    reader.next_frame(&mut buf).unwrap();
    assert_eq!(buf[0], 7);
}

#[test]
fn webp_exif_chunk_is_stripped() {
    let exif = sample::gps(35.0, 139.0, None);
    let mut body = b"WEBP".to_vec();
    body.extend(b"EXIF");
    body.extend((exif.len() as u32).to_le_bytes());
    body.extend(&exif);
    let mut webp = b"RIFF".to_vec();
    webp.extend((body.len() as u32).to_le_bytes());
    webp.extend(body);
    assert!(strip(&mut webp).unwrap().gps);
    assert_eq!(location(&webp[20..]), None);
}

#[test]
fn tiff_files_and_tiff_raws_are_stripped_in_place() {
    let mut tiff = sample::gps(-33.86, 151.21, None);
    assert!(strip(&mut tiff).unwrap().gps);
    assert_eq!(location(&tiff), None);
}

#[test]
fn fits_site_cards_become_blank_cards() {
    let card = |s: &str| format!("{s:<80}");
    let header = [
        card("SIMPLE  =                    T"),
        card("OBJECT  = 'M42     '"),
        card("SITELAT = '35 56 30'"),
        card("SITELONG= '138 28 30'"),
        card("END"),
        card("SITELAT = 'after END is data'"),
    ]
    .concat();
    let mut bytes = header.into_bytes();
    assert_eq!(strip(&mut bytes).unwrap().fits_cards, 2);
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("OBJECT  = 'M42"));
    assert_eq!(text.matches("SITE").count(), 1);
    assert_eq!(&text[160..240], " ".repeat(80));
}

#[test]
fn files_without_a_location_are_left_unchanged() {
    let mut bytes = jpeg(&sample::orientation(6));
    let xmp_free: Vec<u8> = bytes.clone();
    let mut plain = vec![0xFF, 0xD8];
    plain.extend(segment(
        0xE1,
        &[b"Exif\0\0".as_slice(), &sample::orientation(6)].concat(),
    ));
    let copy = plain.clone();
    assert!(!strip(&mut plain).unwrap().any());
    assert_eq!(plain, copy);
    assert!(strip(&mut bytes).unwrap().xmp > 0);
    assert_ne!(bytes, xmp_free);
    let mut gif = b"GIF89a".to_vec();
    assert!(!strip(&mut gif).unwrap().any());
    let mut dicom = vec![0; 128];
    dicom.extend(b"DICM");
    assert!(!strip(&mut dicom).unwrap().any());
}

#[test]
fn unknown_formats_are_reported() {
    let mut heic = b"\0\0\0\x18ftypheic".to_vec();
    assert!(strip(&mut heic).is_err());
}
