use super::*;

fn fixture() -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/hdf4_cases.hdf");
    std::fs::read(&path).unwrap()
}

fn dataset<'a>(c: &'a Contents, name: &str) -> &'a Sds {
    c.datasets
        .iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("{name} not found"))
}

fn numbers(v: &Values) -> Vec<f64> {
    v.bytes
        .chunks_exact(v.kind.size())
        .map(|b| {
            Values {
                kind: v.kind,
                bytes: b.to_vec(),
            }
            .first()
            .unwrap()
        })
        .collect()
}

/// 4 × 5 の式の値。
fn grid(f: impl Fn(f64, f64) -> f64) -> Vec<f64> {
    (0..20).map(|i| f((i / 5) as f64, (i % 5) as f64)).collect()
}

#[test]
fn every_dataset_written_through_the_sd_api_is_found_by_name() {
    let c = read(&fixture(), usize::MAX).unwrap();
    let names: Vec<&str> = c.datasets.iter().map(|d| d.name.as_str()).collect();
    for n in [
        "calibrated",
        "deflated",
        "rle",
        "skphuff",
        "chunked",
        "chunked_plain",
        "cube",
        "Radiance",
        "Latitude",
        "Longitude",
    ] {
        assert!(names.contains(&n), "{n} in {names:?}");
    }
}

#[test]
fn dimension_names_and_sizes_come_from_the_file() {
    let c = read(&fixture(), usize::MAX).unwrap();
    let d = dataset(&c, "calibrated");
    assert_eq!(d.dims, [("row".to_owned(), 4), ("col".to_owned(), 5)]);
    assert_eq!(dataset(&c, "cube").dims.len(), 3);
}

#[test]
fn uncompressed_deflate_rle_and_huffman_data_are_read() {
    let c = read(&fixture(), usize::MAX).unwrap();
    assert_eq!(
        numbers(&dataset(&c, "deflated").data),
        grid(|r, c| r + 0.5 * c)
    );
    assert_eq!(
        numbers(&dataset(&c, "rle").data),
        grid(|r, c| (r + c) % 3.0)
    );
    assert_eq!(
        numbers(&dataset(&c, "skphuff").data),
        grid(|r, c| 1000.0 * r + c)
    );
    assert_eq!(
        numbers(&dataset(&c, "Latitude").data),
        grid(|r, _| 30.0 + r)
    );
}

#[test]
fn chunks_are_put_together_including_partial_edge_chunks() {
    let c = read(&fixture(), usize::MAX).unwrap();
    let want: Vec<f64> = (0..64).map(f64::from).collect();
    assert_eq!(numbers(&dataset(&c, "chunked").data), want);
    let want: Vec<f64> = (0..64).map(|i| -f64::from(i)).collect();
    assert_eq!(numbers(&dataset(&c, "chunked_plain").data), want);
}

#[test]
fn attributes_of_datasets_and_the_file_are_read() {
    let c = read(&fixture(), usize::MAX).unwrap();
    let title = c.attrs.iter().find(|(n, _)| n == "title").unwrap();
    assert_eq!(title.1.bytes, b"vw HDF4 test");
    let d = dataset(&c, "calibrated");
    let units = d.attrs.iter().find(|(n, _)| n == "units").unwrap();
    assert_eq!(units.1.bytes, b"K");
}

#[test]
fn hdf4_calibration_is_turned_into_the_cf_offset() {
    let c = read(&fixture(), usize::MAX).unwrap();
    let d = dataset(&c, "calibrated");
    let get = |n: &str| {
        d.attrs
            .iter()
            .find(|(a, _)| a == n)
            .unwrap()
            .1
            .first()
            .unwrap()
    };
    // 0.5 × (格納値 − 10) = 0.5 × 格納値 − 5
    assert_eq!(get("scale_factor"), 0.5);
    assert_eq!(get("add_offset"), -5.0);
    assert_eq!(get(HDF4_ADD_OFFSET), 10.0);
}

#[test]
fn data_over_the_limit_is_not_read() {
    let c = read(&fixture(), 100).unwrap();
    assert!(c.datasets.len() < 10);
}

#[test]
fn other_files_are_not_hdf4() {
    assert!(read(b"CDF\x01", usize::MAX).is_err());
    assert!(is_hdf4(&fixture()));
}
