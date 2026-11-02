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
fn hdf5_with_neither_a_netcdf_nor_an_hdf5_extension_is_not_opened() {
    let data = fixture("era5_like.nc");
    let plugin = NetcdfPlugin::default();
    assert!(
        plugin
            .decode_frame_at(Path::new("model.weights"), &data, 0)
            .is_err()
    );
    assert!(plugin.decode_frame_at(Path::new("a.NC"), &data, 0).is_ok());
    assert!(plugin.decode_frame_at(Path::new("a.H5"), &data, 0).is_ok());
}

#[test]
fn hdf5_extensions_follow_the_setting() {
    let extensions = Hdf5Extensions::default();
    let plugin = NetcdfPlugin::default().with_hdf5_extensions(extensions.clone());
    for name in ["a.h5", "a.HDF5", "swath.he5"] {
        assert!(plugin.claims_path(Path::new(name)), "{name}");
    }
    assert!(!plugin.claims_path(Path::new("a.hdf")));
    assert!(!plugin.claims_path(Path::new("h5")));
    extensions.set(vw_core::ProbeExtensions::new("^hdf$").unwrap());
    assert!(plugin.claims_path(Path::new("a.hdf")));
    assert!(!plugin.claims_path(Path::new("a.h5")));
    let data = fixture("hdf5_cases.h5");
    assert!(plugin.decode_frame_at(Path::new("a.h5"), &data, 0).is_err());
}

#[test]
fn plain_hdf5_is_labelled_hdf5_and_netcdf4_stays_netcdf4() {
    let plugin = NetcdfPlugin::default();
    let h5 = plugin.decode(&fixture("hdf5_cases.h5")).unwrap();
    assert_eq!(h5.metadata.get(GROUP, FORMAT), Some(FORMAT_HDF5));
    let nc = plugin.decode(&fixture("era5_like.nc")).unwrap();
    assert_eq!(nc.metadata.get(GROUP, FORMAT), Some("NetCDF-4"));
}

const SWATH: &str = "HDFEOS/SWATHS/S/Data Fields/Radiance";

#[test]
fn hdf5_datasets_in_groups_are_images_and_enums_and_compounds_are_not() {
    let paths: Vec<String> = variables(&fixture("hdf5_cases.h5"))
        .unwrap()
        .into_iter()
        .map(|v| v.path)
        .collect();
    assert!(paths.contains(&SWATH.to_owned()));
    assert!(paths.contains(&"cube".to_owned()));
    assert!(!paths.iter().any(|p| p == "kind" || p == "obs"));
}

#[test]
fn hdf5_values_are_scaled_and_the_fill_value_is_missing() {
    let data = fixture("hdf5_cases.h5");
    let img = plugin_for(&data, SWATH).decode(&data).unwrap();
    let v = values(&img);
    assert!(v[0].is_nan());
    // 1 行 2 列: (1000 + 10 + 2) × 0.01
    assert!((v[5 + 2] - 10.12).abs() < 1e-4, "{}", v[7]);
}

#[test]
fn lzf_compressed_datasets_are_read() {
    let data = fixture("hdf5_cases.h5");
    let img = plugin_for(&data, "lzf_flags").decode(&data).unwrap();
    let want: Vec<f32> = (0..256).map(|i| (i % 7) as f32).collect();
    assert_eq!(values(&img), want.as_slice());
}

#[test]
fn three_dimensional_hdf5_datasets_are_planes() {
    let data = fixture("hdf5_cases.h5");
    let plugin = plugin_for(&data, "cube");
    assert_eq!(plugin.frame_count(&data).unwrap(), 3);
    let img = plugin.decode_frame(&data, 2).unwrap();
    assert_eq!(values(&img)[0], 200.0);
}

#[test]
fn latitude_and_longitude_in_the_geolocation_fields_give_each_pixel_its_position() {
    let data = fixture("hdf5_cases.h5");
    let img = geo_of(&data, SWATH, Options::default());
    let geo = img.geo.as_ref().expect("geo");
    assert_eq!(geo.at(0.0, 0.0), Some((30.0, 130.0)));
    assert_eq!(geo.at(2.0, 3.0), Some((33.0, 132.0)));
    // 経度の -999 は欠損。
    assert_eq!(geo.at(4.0, 3.0), None);
}

#[test]
fn a_measured_dataset_is_shown_before_a_flag_dataset_of_the_same_size() {
    // CloudFlag（flag_meanings あり）は Radiance より先に並ぶ。
    let swath: Vec<Variable> = variables(&fixture("hdf5_cases.h5"))
        .unwrap()
        .into_iter()
        .filter(|v| v.path.starts_with("HDFEOS/"))
        .collect();
    assert_eq!(layout::default_variable(&swath).unwrap().path, SWATH);
}

#[test]
fn latitude_and_longitude_datasets_are_not_shown_first() {
    let data = fixture("hdf5_cases.h5");
    let variables = variables(&data).unwrap();
    let auxiliary: Vec<&str> = variables
        .iter()
        .filter(|v| v.auxiliary)
        .map(|v| v.path.as_str())
        .collect();
    assert_eq!(
        auxiliary,
        [
            "HDFEOS/SWATHS/S/Geolocation Fields/Latitude",
            "HDFEOS/SWATHS/S/Geolocation Fields/Longitude"
        ]
    );
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

/// geo-expected.txt の `変数 行,列 緯度 経度` の行。
fn geo_expected(variable: &str) -> Vec<((f64, f64), (f64, f64))> {
    let text = String::from_utf8(fixture("geo-expected.txt")).unwrap();
    text.lines()
        .filter_map(|line| {
            let p: Vec<&str> = line.split_whitespace().collect();
            (p[0] == variable).then(|| {
                let (r, c) = p[1].split_once(',').unwrap();
                (
                    (c.parse().unwrap(), r.parse().unwrap()),
                    (p[2].parse().unwrap(), p[3].parse().unwrap()),
                )
            })
        })
        .collect()
}

fn geo_of(data: &[u8], variable: &str, options: Options) -> VwImage {
    let shared = SharedOptions::default();
    shared.set(options);
    let choices = Choices::default();
    choices.set(data, variable);
    NetcdfPlugin::new(shared, choices).decode(data).unwrap()
}

fn assert_near(got: (f64, f64), want: (f64, f64), tolerance: f64) {
    assert!(
        (got.0 - want.0).abs() < tolerance && (got.1 - want.1).abs() < tolerance,
        "got {got:?}, expected {want:?}"
    );
}

#[test]
fn latitude_and_longitude_axes_give_each_pixel_its_position() {
    let data = fixture("geo_cases.nc");
    let img = geo_of(&data, "lon360", Options::default());
    let geo = img.geo.as_ref().expect("geo");
    assert_eq!(geo.at(0.0, 0.0), Some((30.0, 0.0)));
    assert_eq!(geo.at(2.0, 2.0), Some((-30.0, 90.0)));
    // 225° は西経 135° と出す。
    assert_eq!(geo.at(5.0, 1.0), Some((0.0, -135.0)));
}

#[test]
fn longitudes_from_0_to_360_can_be_rotated_to_start_at_minus_180() {
    let data = fixture("geo_cases.nc");
    let img = geo_of(
        &data,
        "lon360",
        Options {
            rotate_longitude: true,
            ..Options::default()
        },
    );
    // 値は列の番号。180° の列（4 番）が左端に来る。
    assert_eq!(
        &values(&img)[..8],
        &[4.0, 5.0, 6.0, 7.0, 0.0, 1.0, 2.0, 3.0]
    );
    let geo = img.geo.as_ref().unwrap();
    assert_eq!(geo.at(0.0, 0.0), Some((30.0, -180.0)));
    assert_eq!(geo.at(4.0, 0.0), Some((30.0, 0.0)));
    assert_eq!(
        img.metadata.get(GROUP, LONGITUDES),
        Some(LONGITUDES_ROTATED)
    );
}

#[test]
fn longitudes_are_not_rotated_unless_asked() {
    let data = fixture("geo_cases.nc");
    let img = geo_of(&data, "lon360", Options::default());
    assert_eq!(values(&img)[0], 0.0);
    assert_eq!(img.metadata.get(GROUP, LONGITUDES), None);
}

#[test]
fn turning_north_up_also_turns_the_latitudes() {
    let data = fixture("edge_cases.nc");
    let img = plugin_for(&data, "south_up").decode(&data).unwrap();
    let geo = img.geo.as_ref().unwrap();
    let top = geo.at(0.0, 0.0).unwrap().0;
    let bottom = geo.at(0.0, 4.0).unwrap().0;
    assert!(top > bottom, "{top} {bottom}");
}

#[test]
fn curvilinear_grids_read_the_latitude_and_longitude_tables() {
    let data = fixture("geo_cases.nc");
    let img = geo_of(&data, "curvilinear", Options::default());
    let geo = img.geo.as_ref().expect("geo");
    for ((x, y), want) in geo_expected("curvilinear") {
        assert_near(geo.at(x, y).unwrap(), want, 1e-5);
    }
}

#[test]
fn projected_grids_are_turned_into_latitude_and_longitude() {
    let data = fixture("geo_cases.nc");
    let img = geo_of(&data, "lcc_field", Options::default());
    let geo = img.geo.as_ref().expect("geo");
    let cases = geo_expected("lcc_field");
    assert_eq!(cases.len(), 3);
    for ((x, y), want) in cases {
        // 表は f32 で持つので 1e-4 度（約 10 m）まで。
        assert_near(geo.at(x, y).unwrap(), want, 1e-4);
    }
}

#[test]
fn rotated_pole_grids_are_turned_into_latitude_and_longitude() {
    let data = fixture("geo_cases.nc");
    let img = geo_of(&data, "rotated_field", Options::default());
    let geo = img.geo.as_ref().expect("geo");
    for ((x, y), want) in geo_expected("rotated_field") {
        assert_near(geo.at(x, y).unwrap(), want, 1e-4);
    }
}

#[test]
fn anomalies_get_zero_in_the_middle_and_a_diverging_color_map() {
    let data = fixture("geo_cases.nc");
    let img = geo_of(&data, "t_anomaly", Options::default());
    let (black, white) = img.window.unwrap();
    assert!((black + white).abs() < 1e-3, "{black} {white}");
    assert_eq!(img.metadata.get(GROUP, COLOR_MAP), Some(SUGGESTED_PALETTE));
}

#[test]
fn anomalies_keep_the_usual_black_and_white_when_switched_off() {
    let data = fixture("geo_cases.nc");
    let img = geo_of(
        &data,
        "t_anomaly",
        Options {
            auto_anomaly: false,
            ..Options::default()
        },
    );
    let (black, white) = img.window.unwrap();
    assert!((black + white).abs() > 1.0, "{black} {white}");
    assert_eq!(img.metadata.get(GROUP, COLOR_MAP), None);
}

#[test]
fn variables_in_groups_use_the_latitudes_and_longitudes_of_the_parent_group() {
    let data = fixture("sst_groups.nc");
    let img = plugin_for(&data, "quality/quality_level")
        .decode(&data)
        .unwrap();
    assert!(img.geo.is_some());
}

#[test]
fn the_extent_follows_the_edge_of_the_default_variable() {
    let e = extent(&fixture("geo_cases.nc")).expect("extent");
    assert_eq!(e.outline.len(), 4 * 16);
    assert!(
        e.outline
            .iter()
            .all(|&(lat, _)| (-90.0..=90.0).contains(&lat))
    );
    assert!((-90.0..=90.0).contains(&e.center.0));
}

#[test]
fn files_that_cannot_be_read_have_no_extent() {
    assert_eq!(extent(b"not a NetCDF file"), None);
}

#[test]
fn two_dimensional_latitudes_and_longitudes_are_not_shown_first() {
    let variables = variables(&fixture("geo_cases.nc")).unwrap();
    let lat = variables
        .iter()
        .find(|v| v.path == "lat2d")
        .expect("listed");
    assert!(lat.auxiliary);
    let first = layout::default_variable(&variables).unwrap();
    assert!(!first.auxiliary, "{}", first.path);
}
