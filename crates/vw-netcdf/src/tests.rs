use super::*;

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `name` の `variable` を選んだプラグイン。
fn plugin_for(data: &[u8], variable: &str) -> NetcdfPlugin {
    let plugin = NetcdfPlugin::default();
    plugin.choices.set(data, variable);
    plugin
}

fn values(img: &VwImage) -> &[f32] {
    match &img.data {
        PixelData::F32(v) => v,
        _ => panic!("not F32"),
    }
}

/// expected-values.txt の 1 行。
struct Expected {
    file: String,
    variable: String,
    shape: Vec<usize>,
    nan: usize,
    min: f64,
    max: f64,
    mean: f64,
}

fn expected() -> Vec<Expected> {
    let text = String::from_utf8(fixture("expected-values.txt")).unwrap();
    text.lines()
        .map(|line| {
            let mut parts = line.split_whitespace();
            let file = parts.next().unwrap().to_owned();
            let variable = parts.next().unwrap().to_owned();
            let field: HashMap<&str, &str> = parts.filter_map(|p| p.split_once('=')).collect();
            Expected {
                file,
                variable,
                shape: field["shape"]
                    .split('x')
                    .map(|s| s.parse().unwrap())
                    .collect(),
                nan: field["nan"].parse().unwrap(),
                min: field["min"].parse().unwrap(),
                max: field["max"].parse().unwrap(),
                mean: field["mean"].parse().unwrap(),
            }
        })
        .collect()
}

#[test]
fn every_variable_reads_the_same_values_as_the_python_netcdf_library() {
    let all = expected();
    assert_eq!(all.len(), 11);
    for e in all {
        let data = fixture(&e.file);
        let plugin = plugin_for(&data, &e.variable);
        let frames = plugin.frame_count(&data).unwrap();
        let plane = e.shape[e.shape.len() - 2..].iter().product::<usize>();
        assert_eq!(
            frames,
            e.shape.iter().product::<usize>() / plane,
            "{} {}",
            e.file,
            e.variable
        );
        let mut all = Vec::new();
        for f in 0..frames {
            let img = plugin.decode_frame(&data, f).unwrap();
            assert_eq!(img.metadata.get(GROUP, VARIABLE), Some(e.variable.as_str()));
            all.extend_from_slice(values(&img));
        }
        let finite: Vec<f64> = all
            .iter()
            .filter(|v| !v.is_nan())
            .map(|&v| f64::from(v))
            .collect();
        let what = format!("{} {}", e.file, e.variable);
        assert_eq!(all.len() - finite.len(), e.nan, "{what}: missing values");
        let min = finite.iter().copied().fold(f64::INFINITY, f64::min);
        let max = finite.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let mean = finite.iter().sum::<f64>() / finite.len() as f64;
        for (name, got, want) in [
            ("min", min, e.min),
            ("max", max, e.max),
            ("mean", mean, e.mean),
        ] {
            assert!(
                (got - want).abs() <= 1e-4 * want.abs().max(1.0),
                "{what}: {name} {got} != {want}"
            );
        }
    }
}

#[test]
fn netcdf3_and_netcdf4_headers_are_recognised() {
    for name in ["classic.nc", "classic_64bit.nc", "era5_like.nc"] {
        assert!(is_netcdf(&fixture(name)), "{name}");
    }
    assert!(!is_netcdf(b"SIMPLE  =                    T"));
}

#[test]
fn hdf5_without_a_netcdf_extension_is_not_opened() {
    let data = fixture("era5_like.nc");
    let plugin = NetcdfPlugin::default();
    assert!(plugin.decode_frame_at(Path::new("a.h5"), &data, 0).is_err());
    assert!(plugin.decode_frame_at(Path::new("a.NC"), &data, 0).is_ok());
}

#[test]
fn coordinate_and_bounds_variables_are_not_images() {
    let paths: Vec<String> = variables(&fixture("era5_like.nc"))
        .unwrap()
        .into_iter()
        .map(|v| v.path)
        .collect();
    assert_eq!(paths, ["t2m", "tp"]);
}

#[test]
fn variables_inside_groups_are_named_by_their_path() {
    let paths: Vec<String> = variables(&fixture("sst_groups.nc"))
        .unwrap()
        .into_iter()
        .map(|v| v.path)
        .collect();
    assert_eq!(
        paths,
        [
            "sea_surface/analysed_sst",
            "sea_surface/sea_ice_fraction",
            "quality/quality_level"
        ]
    );
}

#[test]
fn time_planes_are_labelled_with_the_date() {
    let data = fixture("era5_like.nc");
    let plugin = plugin_for(&data, "t2m");
    let label = |f| {
        plugin
            .decode_frame(&data, f)
            .unwrap()
            .metadata
            .get(GROUP, vw_fits::PLANE)
            .map(str::to_owned)
    };
    assert_eq!(label(0).as_deref(), Some("time = 2024-01-18"));
    assert_eq!(label(1).as_deref(), Some("time = 2024-01-18 12:00"));
}

#[test]
fn a_two_dimensional_variable_has_one_plane_without_a_label() {
    let data = fixture("classic.nc");
    let plugin = plugin_for(&data, "t2m");
    assert_eq!(plugin.frame_count(&data).unwrap(), 1);
    let img = plugin.decode(&data).unwrap();
    assert_eq!(img.metadata.get(GROUP, vw_fits::PLANE), None);
}

#[test]
fn latitudes_going_south_to_north_are_turned_so_north_is_up() {
    let data = fixture("edge_cases.nc");
    let img = plugin_for(&data, "south_up").decode(&data).unwrap();
    let v = values(&img);
    // ファイルの最初の行（南の端）は 0〜3。北を上にすると最後の行に来る。
    assert_eq!((img.width, img.height), (4, 5));
    assert_eq!(&v[..4], &[40.0, 41.0, 42.0, 43.0]);
    assert_eq!(&v[16..], &[0.0, 1.0, 2.0, 3.0]);
    assert_eq!(img.metadata.get(GROUP, ROWS), Some(ROWS_FLIPPED));
}

#[test]
fn latitudes_going_north_to_south_stay_as_they_are() {
    let data = fixture("classic.nc");
    let img = plugin_for(&data, "t2m").decode(&data).unwrap();
    assert_eq!(img.metadata.get(GROUP, ROWS), None);
}

#[test]
fn turning_north_up_can_be_switched_off() {
    let data = fixture("edge_cases.nc");
    let options = SharedOptions::default();
    options.set(Options {
        north_up: false,
        ..Options::default()
    });
    let choices = Choices::default();
    choices.set(&data, "south_up");
    let img = NetcdfPlugin::new(options, choices).decode(&data).unwrap();
    assert_eq!(&values(&img)[..4], &[0.0, 1.0, 2.0, 3.0]);
}

#[test]
fn longitude_before_latitude_is_read_with_longitude_across() {
    let data = fixture("edge_cases.nc");
    let a = plugin_for(&data, "south_up").decode(&data).unwrap();
    let b = plugin_for(&data, "lon_lat").decode(&data).unwrap();
    assert_eq!((b.width, b.height), (4, 5));
    assert_eq!(values(&a), values(&b));
}

#[test]
fn valid_range_in_the_packed_type_is_compared_with_measured_values() {
    let data = fixture("edge_cases.nc");
    let img = plugin_for(&data, "measured_range").decode(&data).unwrap();
    let v = values(&img);
    // 格納値 0〜27 を 0.5 倍した値のうち、10 を超えるものだけが欠損。
    assert!(v.iter().filter(|x| !x.is_nan()).all(|&x| x <= 10.0));
    assert_eq!(v.iter().filter(|x| x.is_nan()).count(), 5);
}

#[test]
fn valid_range_in_the_stored_type_is_compared_with_stored_values() {
    let data = fixture("edge_cases.nc");
    let img = plugin_for(&data, "stored_range").decode(&data).unwrap();
    let v = values(&img);
    assert!(v.iter().filter(|x| !x.is_nan()).all(|&x| x <= 5.0));
}

#[test]
fn unsigned_bytes_are_read_as_unsigned() {
    let data = fixture("sst_groups.nc");
    let img = plugin_for(&data, "sea_surface/sea_ice_fraction")
        .decode(&data)
        .unwrap();
    assert!(values(&img).iter().all(|&v| (0.0..=1.0).contains(&v)));
}

#[test]
fn packed_values_keep_their_scale_and_offset_in_the_metadata() {
    let data = fixture("era5_like.nc");
    let img = plugin_for(&data, "t2m").decode(&data).unwrap();
    assert_eq!(img.metadata.get(GROUP, SCALE_FACTOR), Some("0.0016"));
    assert_eq!(img.metadata.get(GROUP, ADD_OFFSET), Some("270"));
    assert_eq!(img.metadata.get(GROUP, UNITS), Some("K"));
    assert_eq!(img.metadata.get(GROUP, TYPE), Some("short"));
}

#[test]
fn missing_values_get_the_missing_colour() {
    let data = fixture("era5_like.nc");
    let img = plugin_for(&data, "t2m").decode(&data).unwrap();
    assert_eq!(img.missing, Some(vw_fits::MISSING_COLOR));
    assert!(values(&img).iter().any(|v| v.is_nan()));
}

#[test]
fn the_first_variable_shown_is_the_one_with_the_largest_plane() {
    let data = fixture("classic.nc");
    let img = NetcdfPlugin::default().decode(&data).unwrap();
    assert_eq!(img.metadata.get(GROUP, VARIABLE), Some("t2m"));
}

#[test]
fn all_planes_of_a_variable_share_one_black_and_white() {
    let data = fixture("era5_like.nc");
    let plugin = plugin_for(&data, "tp");
    let a = plugin.decode_frame(&data, 0).unwrap().window;
    let b = plugin.decode_frame(&data, 1).unwrap().window;
    assert!(a.is_some());
    assert_eq!(a, b);
}

#[test]
fn the_variable_list_and_global_attributes_are_in_the_metadata() {
    let data = fixture("era5_like.nc");
    let img = NetcdfPlugin::default().decode(&data).unwrap();
    assert_eq!(img.metadata.get(GROUP, FORMAT), Some("NetCDF-4"));
    assert_eq!(
        img.metadata.get(VARIABLES_GROUP, "tp"),
        Some("time=2, latitude=37, longitude=72 float [m]")
    );
    assert_eq!(
        img.metadata.get(GLOBAL_GROUP, "Conventions"),
        Some("CF-1.6")
    );
    assert_eq!(
        img.metadata.get(VARIABLE_GROUP, "long_name"),
        Some("2 metre temperature")
    );
}

#[test]
fn a_choice_for_a_variable_that_is_not_there_falls_back_to_the_default() {
    let data = fixture("classic.nc");
    let img = plugin_for(&data, "nothing").decode(&data).unwrap();
    assert_eq!(img.metadata.get(GROUP, VARIABLE), Some("t2m"));
}
