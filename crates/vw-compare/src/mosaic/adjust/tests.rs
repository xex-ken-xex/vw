use super::*;
use crate::linalg::apply3;
use crate::mosaic::{LinkKind, tree};

const SIZE: [usize; 2] = [640, 480];
const FOCAL: f64 = 500.0;

/// 向き `yaw`（ラジアン、右が正）・`pitch`（上が正）のカメラの、世界 → カメラの回転。
fn camera(yaw: f64, pitch: f64) -> Mat3 {
    mul3(
        &rotation_of([pitch, 0.0, 0.0]),
        &rotation_of([0.0, -yaw, 0.0]),
    )
}

fn project(r: &Mat3, d: [f64; 3]) -> Option<[f64; 2]> {
    let c = apply3(r, d);
    let p = [
        FOCAL * c[0] / c[2] + (SIZE[0] as f64 - 1.0) / 2.0,
        FOCAL * c[1] / c[2] + (SIZE[1] as f64 - 1.0) / 2.0,
    ];
    (c[2] > 0.0
        && p[0] >= 0.0
        && p[1] >= 0.0
        && p[0] <= SIZE[0] as f64 - 1.0
        && p[1] <= SIZE[1] as f64 - 1.0)
        .then_some(p)
}

fn ray(r: &Mat3, p: [f64; 2]) -> [f64; 3] {
    let c = [
        (p[0] - (SIZE[0] as f64 - 1.0) / 2.0) / FOCAL,
        (p[1] - (SIZE[1] as f64 - 1.0) / 2.0) / FOCAL,
        1.0,
    ];
    apply3(&transpose3(r), c)
}

/// その場で回したカメラで撮った画像どうしの、対応する点の組。
fn panorama(cameras: &[Mat3]) -> Vec<Link> {
    let mut links = Vec::new();
    for a in 0..cameras.len() {
        for b in a + 1..cameras.len() {
            let mut points = Vec::new();
            for j in 0..12 {
                for i in 0..16 {
                    let p = [i as f64 * 42.0 + 5.0, j as f64 * 40.0 + 7.0];
                    if let Some(q) = project(&cameras[b], ray(&cameras[a], p)) {
                        points.push([p, q]);
                    }
                }
            }
            if points.len() < 20 {
                continue;
            }
            links.push(Link {
                a,
                b,
                map: estimate::affine(&points).unwrap(),
                correlation: 0.9,
                overlap: 0.3,
                kind: LinkKind::Features,
                points,
            });
        }
    }
    links
}

fn sizes(n: usize) -> Vec<[usize; 2]> {
    vec![SIZE; n]
}

#[test]
fn the_turn_between_two_sets_of_rays_is_found() {
    let r = rotation_of([0.1, 0.5, -0.2]);
    let pairs: Vec<[[f64; 3]; 2]> = [[0.0, 0.0, 1.0], [0.3, -0.2, 1.0], [-0.4, 0.1, 1.0]]
        .iter()
        .map(|u| {
            let u = normalized3(*u).unwrap();
            [u, apply3(&r, u)]
        })
        .collect();
    let got = turn_between(&pairs).unwrap();
    for i in 0..3 {
        for j in 0..3 {
            assert!((got[i][j] - r[i][j]).abs() < 1e-9, "{got:?} {r:?}");
        }
    }
}

#[test]
fn the_focal_length_is_read_from_how_the_pictures_overlap() {
    let cameras: Vec<Mat3> = (0..3).map(|i| camera(i as f64 * 0.5, 0.05)).collect();
    let links = panorama(&cameras);
    let f = estimate_focal(&sizes(3), &links).unwrap();
    assert!((f - FOCAL).abs() < 1.0, "{f}");
}

#[test]
fn a_wide_turn_is_unrolled_onto_a_cylinder_with_its_seams_closed() {
    let cameras: Vec<Mat3> = (0..6).map(|i| camera(i as f64 * 0.6, 0.0)).collect();
    let links = panorama(&cameras);
    let placed = tree(6, &links);
    let span = cylinder_span(&sizes(6), &links, &placed, FOCAL).unwrap();
    // 0.6 ラジアンずつ 5 回（約 172°）と、1 枚の画角（約 65°）。
    assert!((span - 236.0).abs() < 3.0, "{span}");
    let got = adjust(&sizes(6), &links, &placed, Projection::Cylinder, None).unwrap();
    assert!(got.after < 0.05, "{} -> {}", got.before, got.after);
    assert!((got.focal.unwrap() - FOCAL).abs() < 1.0, "{:?}", got.focal);
    // 真ん中の高さの画素は、どの画像でも貼り合わせの同じ高さに来る（水平にまっすぐ）。
    let heights: Vec<f64> = got
        .placed
        .iter()
        .map(|w| w.unwrap().apply([319.5, 239.5]).unwrap()[1])
        .collect();
    for h in &heights {
        assert!((h - heights[0]).abs() < 0.5, "{heights:?}");
    }
}

#[test]
fn a_tilted_turn_is_levelled_on_the_cylinder() {
    // カメラを少し上に向けたまま回した。円筒は傾きを打ち消し、中心の高さがそろう。
    let cameras: Vec<Mat3> = (0..5).map(|i| camera(i as f64 * 0.6, 0.15)).collect();
    let links = panorama(&cameras);
    let placed = tree(5, &links);
    let got = adjust(
        &sizes(5),
        &links,
        &placed,
        Projection::Cylinder,
        Some(FOCAL),
    )
    .unwrap();
    let heights: Vec<f64> = got
        .placed
        .iter()
        .map(|w| w.unwrap().apply([319.5, 239.5]).unwrap()[1])
        .collect();
    for h in &heights {
        assert!((h - heights[0]).abs() < 0.5, "{heights:?}");
    }
}

#[test]
fn a_narrow_turn_is_fitted_with_a_projective_warp() {
    let cameras: Vec<Mat3> = (0..3).map(|i| camera(i as f64 * 0.45, 0.0)).collect();
    let links = panorama(&cameras);
    let placed = tree(3, &links);
    let affine = adjust(&sizes(3), &links, &placed, Projection::Affine, None).unwrap();
    let projective = adjust(&sizes(3), &links, &placed, Projection::Projective, None).unwrap();
    assert!(affine.after > 2.0, "{}", affine.after);
    assert!(projective.after < 0.05, "{}", projective.after);
}

#[test]
fn the_perspective_of_a_turn_is_measured_and_a_flat_shift_has_none() {
    let cameras: Vec<Mat3> = (0..2).map(|i| camera(i as f64 * 0.5, 0.0)).collect();
    let turned: HashMap<(usize, usize), Vec<Pair>> = panorama(&cameras)
        .into_iter()
        .map(|l| ((l.a, l.b), l.points))
        .collect();
    assert!(perspective(&turned).unwrap() > 5.0);
    let shift = Affine {
        c: -300.0,
        ..Affine::IDENTITY
    };
    let flat: Vec<Pair> = (0..40)
        .map(|i| {
            let p = [300.0 + (i % 8) as f64 * 40.0, (i / 8) as f64 * 90.0];
            [p, shift.apply(p)]
        })
        .collect();
    let flat = HashMap::from([((0, 1), flat)]);
    assert!(perspective(&flat).unwrap() < 1e-6);
}

#[test]
fn tiles_chained_around_a_loop_are_pulled_together() {
    // 2 × 2 のタイル。たどる組の 1 つが 3 画素ずれていて、木のままでは輪が閉じない。
    let at = [[0.0, 0.0], [500.0, 0.0], [0.0, 400.0], [500.0, 400.0]];
    let size = [700, 560];
    let link = |a: usize, b: usize, error: f64| {
        let map = Affine {
            c: at[a][0] - at[b][0] + error,
            f: at[a][1] - at[b][1],
            ..Affine::IDENTITY
        };
        Link::by_map((a, size), (b, size), map, 0.9, 0.3, LinkKind::Overlap)
    };
    let links = vec![
        link(0, 1, 0.0),
        link(0, 2, 0.0),
        link(1, 3, 3.0),
        link(2, 3, 0.0),
    ];
    let placed = tree(4, &links);
    let got = adjust(&[size; 4], &links, &placed, Projection::Affine, None).unwrap();
    assert!(got.after < got.before, "{} -> {}", got.before, got.after);
    let pos = |i: usize| got.placed[i].unwrap().apply([0.0, 0.0]).unwrap();
    let origin = pos(0);
    let d = [pos(3)[0] - origin[0], pos(3)[1] - origin[1]];
    // 確かな 3 本の組と食い違う 1 本の真ん中あたりに来る。
    assert!(
        (d[0] - 500.0).abs() < 2.0 && (d[1] - 400.0).abs() < 0.5,
        "{d:?}"
    );
}
