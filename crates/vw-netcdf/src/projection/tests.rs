//! 期待値は PROJ 9（pyproj 3.8）で緯度・経度から x・y に投影した値。逆に直して 1e-6 度以内で戻ればよい。

use super::*;

fn r(deg: f64) -> f64 {
    deg.to_radians()
}

fn check(p: &Projection, cases: &[(f64, f64, f64, f64)]) {
    for &(lat, lon, x, y) in cases {
        let (got_lat, got_lon) = p.inverse(x, y).unwrap();
        assert!(
            (got_lat - lat).abs() < 1e-6 && (got_lon - lon).abs() < 1e-6,
            "{p:?}: ({x}, {y}) -> ({got_lat}, {got_lon}), expected ({lat}, {lon})"
        );
    }
}

#[test]
fn lambert_conformal_conic_with_two_parallels() {
    let p = Projection::LambertConformalConic {
        ellipsoid: Ellipsoid::WGS84,
        parallels: (r(30.0), r(60.0)),
        origin_lat: r(35.0),
        central_lon: r(135.0),
        false_easting: 1000.0,
        false_northing: -2000.0,
    };
    check(
        &p,
        &[
            (35.68, 139.77, 423863.2355806754, 84597.99938440992),
            (50.0, 120.0, -1034538.9251438195, 1711348.512232046),
            (20.0, 150.0, 1651623.4491791106, -1533009.2546927584),
        ],
    );
}

#[test]
fn lambert_conformal_conic_on_a_sphere_with_one_parallel() {
    let p = Projection::LambertConformalConic {
        ellipsoid: Ellipsoid::sphere(6_371_229.0),
        parallels: (r(25.0), r(25.0)),
        origin_lat: r(25.0),
        central_lon: r(-95.0),
        false_easting: 0.0,
        false_northing: 0.0,
    };
    check(
        &p,
        &[
            (30.0, -100.0, -483261.028426781, 565621.4935718568),
            (45.0, -80.0, 1257671.2651160539, 2342451.036573874),
            (10.0, -120.0, -2814546.4739387743, -1426560.552377235),
        ],
    );
}

#[test]
fn north_polar_stereographic_with_a_standard_parallel() {
    let p = Projection::PolarStereographic {
        ellipsoid: Ellipsoid::WGS84,
        north: true,
        central_lon: r(-105.0),
        scale: PolarScale::StandardParallel(r(60.0)),
        false_easting: 0.0,
        false_northing: 0.0,
    };
    check(
        &p,
        &[
            (60.0, -30.0, 3088165.889857085, -827471.5562805446),
            (80.0, 100.0, -441521.40866698395, 946845.7164394747),
            (45.0, -105.0, 0.0, -4937018.428368093),
        ],
    );
}

#[test]
fn south_polar_stereographic_with_a_scale_at_the_pole() {
    let p = Projection::PolarStereographic {
        ellipsoid: Ellipsoid::WGS84,
        north: false,
        central_lon: 0.0,
        scale: PolarScale::ScaleAtPole(0.97),
        false_easting: 0.0,
        false_northing: 0.0,
    };
    check(
        &p,
        &[
            (-70.0, 30.0, 1094123.7806562788, 1895077.9778660215),
            (-60.0, -150.0, -1661823.0864437474, -2878362.0189114977),
            (-85.0, 90.0, 542045.9453130759, 0.0),
        ],
    );
}

#[test]
fn polar_stereographic_true_at_the_pole() {
    let p = Projection::PolarStereographic {
        ellipsoid: Ellipsoid::WGS84,
        north: true,
        central_lon: r(10.0),
        scale: PolarScale::StandardParallel(r(90.0)),
        false_easting: 0.0,
        false_northing: 0.0,
    };
    check(
        &p,
        &[
            (70.0, 0.0, -391737.32093405165, -2221652.746279035),
            (85.0, -170.0, 0.0, 558810.2529000783),
        ],
    );
}

#[test]
fn mercator_with_a_standard_parallel() {
    let el = Ellipsoid::WGS84;
    let p = Projection::Mercator {
        ellipsoid: el,
        central_lon: r(120.0),
        k0: el.m(r(20.0)),
        false_easting: 0.0,
        false_northing: 0.0,
    };
    check(
        &p,
        &[
            (10.0, 100.0, -2092941.7261972378, 1044854.1418091677),
            (-30.0, 140.0, 2092941.726197239, -3273469.355377555),
            (60.0, 170.0, 5232354.315493099, 7861444.842695906),
        ],
    );
}

#[test]
fn transverse_mercator_like_utm_zone_54() {
    let p = Projection::TransverseMercator {
        ellipsoid: Ellipsoid::WGS84,
        origin_lat: 0.0,
        central_lon: r(141.0),
        k0: 0.9996,
        false_easting: 500000.0,
        false_northing: 0.0,
    };
    check(
        &p,
        &[
            (35.68, 139.77, 388694.1453257633, 3949153.62270427),
            (43.0, 142.0, 581508.6478303487, 4761299.925223492),
            (30.0, 140.5, 451775.8487734758, 3318890.5630454323),
        ],
    );
}

#[test]
fn albers_equal_area_conic() {
    let p = Projection::AlbersEqualArea {
        ellipsoid: Ellipsoid::WGS84,
        parallels: (r(29.5), r(45.5)),
        origin_lat: r(23.0),
        central_lon: r(-96.0),
        false_easting: 0.0,
        false_northing: 0.0,
    };
    check(
        &p,
        &[
            (35.0, -75.0, 1885428.390532334, 1535969.285828168),
            (45.0, -110.0, -1098421.3842696198, 2526775.379615174),
            (25.0, -90.0, 612693.2617314962, 237467.83769956493),
        ],
    );
}

#[test]
fn rotated_pole_grid_goes_back_to_geographic() {
    // COSMO-EU に似た格子の北極（39.25°N, 162°W）。
    for (lat, lon, rlat, rlon) in [
        (50.0, 10.0, -0.4724280878272969, -5.132644799516191),
        (40.0, -5.0, -8.011160504972242, -17.593830651330087),
        (60.0, 30.0, 9.74152572000314, 6.054632197999295),
    ] {
        let (got_lat, got_lon) = unrotate(rlat, rlon, (39.25, -162.0), 0.0);
        assert!(
            (got_lat - lat).abs() < 1e-6 && (got_lon - lon).abs() < 1e-6,
            "({rlat}, {rlon}) -> ({got_lat}, {got_lon}), expected ({lat}, {lon})"
        );
    }
}
