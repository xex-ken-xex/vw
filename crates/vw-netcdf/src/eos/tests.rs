//! 図法の期待値は PROJ 9（pyproj 3.8）で緯度・経度から x・y に投影した値。逆に直して 1e-6 度以内で戻ればよい。

use super::*;

/// MODIS の雲（MOD06）と陸の反射率（MCD43）に似せた構造の文字。
const MODIS_LIKE: &str = r#"GROUP=SwathStructure
	GROUP=SWATH_1
		SwathName="mod06"
		GROUP=Dimension
			OBJECT=Dimension_1
				DimensionName="Cell_Along_Swath_5km"
				Size=406
			END_OBJECT=Dimension_1
		END_GROUP=Dimension
		GROUP=DimensionMap
			OBJECT=DimensionMap_1
				GeoDimension="Cell_Across_Swath_5km"
				DataDimension="Cell_Across_Swath_1km"
				Offset=2
				Increment=5
			END_OBJECT=DimensionMap_1
			OBJECT=DimensionMap_2
				GeoDimension="Cell_Along_Swath_5km"
				DataDimension="Cell_Along_Swath_1km"
				Offset=2
				Increment=5
			END_OBJECT=DimensionMap_2
		END_GROUP=DimensionMap
		GROUP=GeoField
			OBJECT=GeoField_1
				GeoFieldName="Latitude"
				DataType=DFNT_FLOAT32
				DimList=("Cell_Along_Swath_5km","Cell_Across_Swath_5km")
			END_OBJECT=GeoField_1
			OBJECT=GeoField_2
				GeoFieldName="Longitude"
				DataType=DFNT_FLOAT32
				DimList=("Cell_Along_Swath_5km",
					"Cell_Across_Swath_5km")
			END_OBJECT=GeoField_2
		END_GROUP=GeoField
	END_GROUP=SWATH_1
END_GROUP=SwathStructure
GROUP=GridStructure
	GROUP=GRID_1
		GridName="MOD_Grid_BRDF"
		XDim=2400
		YDim=2400
		UpperLeftPointMtrs=(-11119505.196667,4447802.078667)
		LowerRightMtrs=(-10007554.677000,3335851.559000)
		Projection=GCTP_SNSOID
		ProjParams=(6371007.181000,0,0,0,0,0,0,0,0,0,0,0,0)
		SphereCode=-1
		GridOrigin=HDFE_GD_UL
	END_GROUP=GRID_1
	GROUP=GRID_2
		GridName="Global_0.5deg"
		XDim=720
		YDim=360
		UpperLeftPointMtrs=(-180000000.000000,90000000.000000)
		LowerRightMtrs=(180000000.000000,-90000000.000000)
		Projection=HE5_GCTP_GEO
		GridOrigin=HE5_HDFE_GD_LL
		PixelRegistration=HE5_HDFE_CORNER
	END_GROUP=GRID_2
END_GROUP=GridStructure
GROUP=PointStructure
END_GROUP=PointStructure
END
"#;

fn grid(projection: &str, params: &[f64], sphere: i32, zone: i32) -> Grid {
    Grid {
        name: "g".into(),
        columns: 2,
        rows: 2,
        upper_left: (0.0, 0.0),
        lower_right: (1.0, -1.0),
        projection: projection.into(),
        params: params.to_vec(),
        sphere,
        zone,
        origin_bottom: false,
        origin_right: false,
        corner: false,
    }
}

fn check(g: &Grid, cases: &[(f64, f64, f64, f64)]) {
    let p = g.projection().unwrap();
    for &(lat, lon, x, y) in cases {
        let (got_lat, got_lon) = p.inverse(x, y).unwrap();
        assert!(
            (got_lat - lat).abs() < 1e-6 && (got_lon - lon).abs() < 1e-6,
            "{p:?}: ({x}, {y}) -> ({got_lat}, {got_lon}), expected ({lat}, {lon})"
        );
    }
}

#[test]
fn reads_dimension_maps_and_geolocation_fields_of_a_swath() {
    let s = parse(MODIS_LIKE);
    assert_eq!(s.swaths.len(), 1);
    let swath = &s.swaths[0];
    assert_eq!(swath.name, "mod06");
    assert_eq!(
        swath.maps[1],
        DimensionMap {
            geo: "Cell_Along_Swath_5km".into(),
            data: "Cell_Along_Swath_1km".into(),
            offset: 2.0,
            increment: 5.0,
        }
    );
    // 2 行にまたがった DimList も 1 つの値として読む。
    assert_eq!(
        swath.geo_fields[1].dims,
        ["Cell_Along_Swath_5km", "Cell_Across_Swath_5km"]
    );
}

#[test]
fn reads_the_projection_and_corners_of_grids_in_both_versions() {
    let s = parse(MODIS_LIKE);
    let modis = &s.grids[0];
    assert_eq!(modis.name, "MOD_Grid_BRDF");
    assert_eq!((modis.columns, modis.rows), (2400, 2400));
    assert_eq!(modis.projection, "SNSOID");
    assert_eq!(modis.upper_left, (-11119505.196667, 4447802.078667));
    assert!(!modis.origin_bottom && !modis.corner);
    let global = &s.grids[1];
    assert_eq!(global.projection, "GEO");
    assert!(global.origin_bottom && !global.origin_right && global.corner);
}

#[test]
fn a_broken_text_keeps_what_could_be_read() {
    let s = parse("GROUP=SwathStructure\nGROUP=SWATH_1\nSwathName=\"a\"\nnonsense line\n");
    assert_eq!(s.swaths.len(), 1);
    assert!(parse("").swaths.is_empty());
}

#[test]
fn packed_degrees_minutes_seconds_become_degrees() {
    assert_eq!(packed_dms(-100_000_000.0), -100.0);
    assert!((packed_dms(45_030_036.0) - (45.0 + 30.0 / 60.0 + 36.0 / 3600.0)).abs() < 1e-12);
    assert!((packed_dms(-12_015_000.0) + 12.25).abs() < 1e-12);
}

#[test]
fn modis_sinusoidal_pixels_are_centered_from_the_upper_left_corner() {
    let g = &parse(MODIS_LIKE).grids[0];
    let (xs, ys) = g.pixel_coordinates();
    let step = (-10007554.677 - -11119505.196667) / 2400.0;
    assert!((xs[0] - (-11119505.196667 + step / 2.0)).abs() < 1e-6);
    assert!(ys[0] < 4447802.078667 && ys[1] < ys[0]);
    check(
        g,
        &[
            (35.0, -100.0, -9108565.414149545, 3891826.819182831),
            (-60.0, 170.0, 9451579.418015448, -6671703.118599138),
            (0.5, 0.25, 27797.704502448436, 55597.52598832616),
        ],
    );
}

#[test]
fn sinusoidal_has_no_position_outside_the_earth() {
    let g = &parse(MODIS_LIKE).grids[0];
    assert_eq!(
        g.projection().unwrap().inverse(-19_000_000.0, 6_000_000.0),
        None
    );
}

#[test]
fn geographic_grid_from_the_lower_left_uses_corners_in_degrees() {
    let g = &parse(MODIS_LIKE).grids[1];
    let (lons, lats) = g.pixel_coordinates();
    assert_eq!(lons[0], -180.0);
    assert_eq!(lons[1], -179.5);
    // 行 0 が下の端。
    assert_eq!(lats[0], -90.0);
    assert_eq!(lats[359], 89.5);
    assert!(g.projection().is_none());
}

#[test]
fn lambert_azimuthal_equal_area_at_the_poles_and_elsewhere() {
    let north = grid(
        "LAMAZ",
        &[6371228.0, 0.0, 0.0, 0.0, 0.0, 90_000_000.0],
        19,
        0,
    );
    check(
        &north,
        &[
            (70.0, 45.0, 1564618.1894112618, -1564618.189411262),
            (50.0, -120.0, -3774291.673543519, 2179088.3137205127),
        ],
    );
    let south = grid(
        "LAMAZ",
        &[6371228.0, 0.0, 0.0, 0.0, 0.0, -90_000_000.0],
        19,
        0,
    );
    check(
        &south,
        &[(-70.0, 45.0, 1564618.1894112625, 1564618.1894112625)],
    );
    let middle = grid(
        "LAMAZ",
        &[
            6371228.0,
            0.0,
            0.0,
            0.0,
            -100_000_000.0,
            40_000_000.0,
            100.0,
            -50.0,
        ],
        19,
        0,
    );
    check(
        &middle,
        &[
            (45.0, -90.0, 784773.4578923299, 601044.7420813728),
            (30.0, -120.0, -1913640.7311055523, -905088.6675908325),
        ],
    );
}

#[test]
fn cylindrical_equal_area_on_a_sphere_and_an_ellipsoid() {
    let sphere = grid("CEA", &[6371228.0, 0.0, 0.0, 0.0, 0.0, 30_000_000.0], 19, 0);
    check(
        &sphere,
        &[
            (45.0, 100.0, 9630107.746492706, 5202085.878310993),
            (-80.0, -170.0, -16371183.169037601, -7245093.161459067),
        ],
    );
    let wgs84 = grid("CEA", &[0.0, 0.0, 0.0, 0.0, 0.0, 30_000_000.0], 12, 0);
    check(
        &wgs84,
        &[
            (45.0, 100.0, 9648628.025089651, 5180102.328839251),
            (-80.0, -170.0, -16402667.642652407, -7229704.847593094),
        ],
    );
}

#[test]
fn utm_zone_comes_from_the_zone_code_and_its_sign_means_south() {
    check(
        &grid("UTM", &[], 12, 54),
        &[(35.68, 139.77, 388694.1453257633, 3949153.62270427)],
    );
    check(
        &grid("UTM", &[], 0, -33),
        &[(-20.0, 16.0, 604610.917222078, 7788337.975800641)],
    );
}

#[test]
fn utm_without_a_zone_code_finds_the_zone_from_the_parameters() {
    let g = grid("UTM", &[139_000_000.0, 35_000_000.0], 12, 0);
    check(&g, &[(35.68, 139.77, 388694.1453257633, 3949153.62270427)]);
}

fn map(offset: f64, increment: f64) -> DimensionMap {
    DimensionMap {
        geo: "g".into(),
        data: "d".into(),
        offset,
        increment,
    }
}

#[test]
fn coarse_geolocation_spreads_to_every_pixel_including_the_edges() {
    // 緯度・経度は 5 画素おき、2 画素目から（MODIS の 5 km と 1 km の対応）。
    let (gw, gh) = (3, 2);
    let coarse: Vec<f64> = (0..gh)
        .flat_map(|r| (0..gw).map(move |c| 10.0 + r as f64 * 5.0 + c as f64 * 0.5))
        .collect();
    let (w, h) = (15, 10);
    let out = interpolate(
        &coarse,
        (gw, gh),
        (w, h),
        (&map(2.0, 5.0), &map(2.0, 5.0)),
        false,
    );
    assert_eq!(out.len(), w * h);
    for j in 0..h {
        for i in 0..w {
            let expected = 10.0 + (j as f64 - 2.0) + (i as f64 - 2.0) * 0.1;
            assert!(
                (f64::from(out[j * w + i]) - expected).abs() < 1e-4,
                "({i}, {j}): {} vs {expected}",
                out[j * w + i]
            );
        }
    }
}

#[test]
fn longitudes_across_the_date_line_do_not_jump() {
    let coarse = [179.0, -179.0, 179.0, -179.0];
    let out = interpolate(
        &coarse,
        (2, 2),
        (3, 2),
        (&map(0.0, 1.0), &map(0.0, 2.0)),
        true,
    );
    // 真ん中の列は 180° のはず（0° にはならない）。
    assert!((f64::from(out[1]).abs() - 180.0).abs() < 1e-4, "{out:?}");
}

#[test]
fn latitudes_pushed_past_a_pole_stop_at_the_pole() {
    let coarse = [80.0, 80.0, 89.0, 89.0];
    let out = interpolate(
        &coarse,
        (2, 2),
        (2, 4),
        (&map(0.0, 1.0), &map(0.0, 1.0)),
        false,
    );
    assert_eq!(out[3 * 2], 90.0);
}

#[test]
fn a_missing_coarse_point_leaves_its_neighbourhood_missing() {
    let coarse = [10.0, f64::NAN, 10.0, 10.0];
    let out = interpolate(
        &coarse,
        (2, 2),
        (2, 2),
        (&map(0.0, 1.0), &map(0.0, 1.0)),
        false,
    );
    assert_eq!(out[0], 10.0);
    assert!(out[1].is_nan());
}

#[test]
fn negative_increment_means_the_geolocation_is_finer() {
    let m = map(1.0, -2.0);
    assert_eq!(m.geo_index(0.0), 1.0);
    assert_eq!(m.geo_index(3.0), 7.0);
}
