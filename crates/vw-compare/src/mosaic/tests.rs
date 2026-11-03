use super::*;

fn warps(p: &[Option<Affine>]) -> Vec<Option<Warp>> {
    p.iter().map(|p| p.map(Warp::Affine)).collect()
}

/// 全体に散らした、ぼけた点の模様（0〜255）。どこを切り出しても似た所のない絵にする。
fn scene(width: usize, height: usize) -> Plane {
    let mut seed = 0x9e37_79b9_u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        f64::from(seed) / f64::from(u32::MAX)
    };
    let count = width * height / 900;
    let blobs: Vec<(f64, f64, f64, f64)> = (0..count)
        .map(|_| {
            (
                next() * width as f64,
                next() * height as f64,
                2.0 + next() * 9.0,
                next() * 2.0 - 1.0,
            )
        })
        .collect();
    let mut data = vec![110.0f32; width * height];
    for &(bx, by, r, amp) in &blobs {
        let reach = (r * 3.0) as isize;
        for dy in -reach..=reach {
            for dx in -reach..=reach {
                let (x, y) = (bx as isize + dx, by as isize + dy);
                if x < 0 || y < 0 || x >= width as isize || y >= height as isize {
                    continue;
                }
                let d2 = (x as f64 - bx).powi(2) + (y as f64 - by).powi(2);
                data[y as usize * width + x as usize] +=
                    (amp * 100.0 * (-d2 / (2.0 * r * r)).exp()) as f32;
            }
        }
    }
    for v in &mut data {
        *v = v.clamp(0.0, 255.0);
    }
    Plane::new(width, height, data)
}

/// `src` の (x, y) から切り出した `w` × `h`。
fn crop(src: &Plane, x: usize, y: usize, w: usize, h: usize) -> Plane {
    let data = (0..w * h).map(|i| src.at(x + i % w, y + i / w)).collect();
    Plane::new(w, h, data)
}

fn piece(plane: Plane) -> Piece {
    Piece {
        full: [plane.width, plane.height],
        plane,
    }
}

fn shift(x: f64, y: f64) -> Affine {
    Affine {
        c: x,
        f: y,
        ..Affine::IDENTITY
    }
}

fn near(got: [f64; 2], want: [f64; 2], tolerance: f64) -> bool {
    (got[0] - want[0]).hypot(got[1] - want[1]) < tolerance
}

#[test]
fn two_crops_sharing_a_third_are_linked_by_their_offset() {
    let s = scene(700, 400);
    let pieces = [crop(&s, 0, 0, 400, 300), crop(&s, 260, 40, 400, 300)].map(piece);
    let links = link_all(&pieces, Search::default(), &|| {});
    assert_eq!(links.len(), 1, "{links:?}");
    let l = &links[0];
    assert!(l.correlation > 0.9, "{l:?}");
    // a の (300, 100) は、b では (40, 60)。
    assert!(
        near(l.map.apply([300.0, 100.0]), [40.0, 60.0], 0.3),
        "{l:?}"
    );
}

#[test]
fn images_that_do_not_overlap_are_not_linked() {
    let s = scene(900, 300);
    let pieces = [crop(&s, 0, 0, 300, 300), crop(&s, 600, 0, 300, 300)].map(piece);
    let links = link_all(&pieces, Search::default(), &|| {});
    assert!(links.is_empty(), "{links:?}");
}

#[test]
fn a_tile_turned_half_way_round_is_linked_when_quarter_turns_are_searched() {
    let s = scene(700, 400);
    let a = crop(&s, 0, 0, 400, 300);
    let b = quarter_turn(&crop(&s, 250, 60, 400, 300), 2);
    let pieces = [piece(a), piece(b)];
    assert!(link_all(&pieces, Search::default(), &|| {}).is_empty());
    let links = link_all(
        &pieces,
        Search {
            quarter_turns: true,
        },
        &|| {},
    );
    assert_eq!(links.len(), 1, "{links:?}");
    // a の (300, 100) は、回す前の b では (50, 40)、半回転した b では (349, 259)。
    assert!(near(
        links[0].map.apply([300.0, 100.0]),
        [349.0, 259.0],
        0.5
    ));
}

#[test]
fn a_grid_of_tiles_is_placed_where_it_was_cut_from() {
    let s = scene(770, 570);
    let at = [(0, 0), (330, 10), (8, 250), (340, 262)];
    let pieces: Vec<Piece> = at
        .iter()
        .map(|&(x, y)| piece(crop(&s, x, y, 420, 300)))
        .collect();
    let links = link_all(&pieces, Search::default(), &|| {});
    let placed = place(pieces.len(), &links);
    let first = placed
        .iter()
        .position(|p| *p == Some(Affine::IDENTITY))
        .unwrap();
    for (i, p) in placed.iter().enumerate() {
        let p = p.expect("every tile is placed");
        let want = [
            at[i].0 as f64 - at[first].0 as f64,
            at[i].1 as f64 - at[first].1 as f64,
        ];
        assert!(near(p.apply([0.0, 0.0]), want, 0.5), "{i}: {p:?}");
    }
    let b = bounds(&[[420, 300]; 4], &warps(&placed)).unwrap();
    assert!(
        (b[2] - b[0] - 760.0).abs() < 1.0 && (b[3] - b[1] - 562.0).abs() < 1.0,
        "{b:?}"
    );
}

#[test]
fn a_tile_with_no_partner_is_left_unplaced() {
    let placed = place(
        3,
        &[Link {
            a: 0,
            b: 1,
            map: shift(-100.0, 0.0),
            correlation: 0.9,
            overlap: 0.5,
            kind: LinkKind::Overlap,
            points: Vec::new(),
        }],
    );
    assert!(placed[0].is_some() && placed[1].is_some());
    assert_eq!(placed[2], None);
}

#[test]
fn shots_of_the_same_area_are_grouped() {
    let sizes = [[400, 300]; 4];
    let placed = [
        Some(Affine::IDENTITY),
        Some(shift(20.0, 10.0)),
        Some(shift(300.0, 0.0)),
        None,
    ];
    assert_eq!(same_area(&sizes, &warps(&placed), 0.8), vec![vec![0, 1]]);
    assert!(same_area(&sizes, &warps(&placed), 0.95).is_empty());
}

#[test]
fn the_share_is_measured_against_the_smaller_image() {
    let inner = shared(
        [100, 100],
        &shift(50.0, 50.0).into(),
        [400, 400],
        &Affine::IDENTITY.into(),
    );
    assert!((inner - 1.0).abs() < 1e-9);
    let half = shared(
        [200, 100],
        &Affine::IDENTITY.into(),
        [200, 100],
        &shift(100.0, 0.0).into(),
    );
    assert!((half - 0.5).abs() < 0.05, "{half}");
}

#[test]
fn feathered_tiles_rebuild_the_scene_without_a_step() {
    let s = scene(500, 300);
    let (a, b) = (crop(&s, 0, 0, 300, 300), crop(&s, 200, 0, 300, 300));
    let grid = Grid::covering([-0.5, -0.5, 499.5, 299.5], 1_000_000).unwrap();
    assert_eq!((grid.width, grid.height), (500, 300));
    let mut canvas = Canvas::new(grid, 1, Blend::Feather);
    for (p, x) in [(&a, 0.0), (&b, 200.0)] {
        canvas.add(&Layer {
            width: p.width,
            height: p.height,
            channels: 1,
            data: &p.data,
            placed: shift(x, 0.0).into(),
            gain: 1.0,
        });
    }
    let out = canvas.finish();
    for (i, (got, want)) in out.iter().zip(&s.data).enumerate() {
        assert!((got - want).abs() < 0.01, "{i}: {got} {want}");
    }
}

#[test]
fn the_last_tile_covers_the_overlap_when_blending_is_off() {
    let grid = Grid::covering([-0.5, -0.5, 3.5, 0.5], 100).unwrap();
    let mut canvas = Canvas::new(grid, 1, Blend::Top);
    for (value, x) in [(10.0f32, 0.0), (20.0, 1.0)] {
        canvas.add(&Layer {
            width: 3,
            height: 1,
            channels: 1,
            data: &[value; 3],
            placed: shift(x, 0.0).into(),
            gain: 1.0,
        });
    }
    assert_eq!(canvas.finish(), vec![10.0, 20.0, 20.0, 20.0]);
}

#[test]
fn what_no_tile_covers_is_left_empty() {
    let grid = Grid::covering([-0.5, -0.5, 3.5, 0.5], 100).unwrap();
    let mut canvas = Canvas::new(grid, 1, Blend::Feather);
    canvas.add(&Layer {
        width: 2,
        height: 1,
        channels: 1,
        data: &[5.0, 5.0],
        placed: Affine::IDENTITY.into(),
        gain: 1.0,
    });
    let out = canvas.finish();
    assert_eq!(&out[..2], &[5.0, 5.0]);
    assert!(out[2].is_nan() && out[3].is_nan());
}

#[test]
fn a_big_mosaic_is_shrunk_to_the_pixel_limit() {
    let grid = Grid::covering([-0.5, -0.5, 3999.5, 1999.5], 2_000_000).unwrap();
    assert!(grid.width * grid.height <= 2_000_000 + 4000);
    assert!((grid.step() - 2.0).abs() < 0.01, "{}", grid.step());
}

#[test]
fn a_darker_tile_gets_a_gain_that_matches_its_neighbour() {
    let s = scene(500, 300);
    let a = crop(&s, 0, 0, 300, 300);
    let mut b = crop(&s, 200, 0, 300, 300);
    for v in &mut b.data {
        *v *= 0.8;
    }
    let pieces = [piece(a), piece(b)];
    let placed = [Some(Affine::IDENTITY), Some(shift(200.0, 0.0))];
    let g = gains(&pieces, &warps(&placed), &[true, true]);
    let ratio = g[1] / g[0];
    assert!((ratio - 1.25).abs() < 0.03, "{g:?}");
}

#[test]
fn a_seam_placed_right_is_clean_and_one_placed_wrong_is_misaligned() {
    let s = scene(700, 400);
    let pieces = [crop(&s, 0, 0, 400, 300), crop(&s, 260, 40, 400, 300)].map(piece);
    let links = link_all(&pieces, Search::default(), &|| {});
    let right = place(2, &links);
    let seams_right = seams(&pieces, &warps(&right), &links, &[true, true], &[1.0, 1.0]);
    assert_eq!(seams_right.len(), 1);
    assert_eq!(seams_right[0].verdict(), Verdict::Clean, "{seams_right:?}");
    let first = right
        .iter()
        .position(|p| *p == Some(Affine::IDENTITY))
        .unwrap();
    let mut wrong = right;
    let other = 1 - first;
    wrong[other] = wrong[other].map(|p| p.then(&shift(12.0, -7.0)));
    let seams_wrong = seams(&pieces, &warps(&wrong), &links, &[true, true], &[1.0, 1.0]);
    assert_eq!(
        seams_wrong[0].verdict(),
        Verdict::Misaligned,
        "{seams_wrong:?}"
    );
}

#[test]
fn a_blurred_tile_makes_its_seam_visible() {
    let s = scene(700, 400);
    let a = crop(&s, 0, 0, 400, 300);
    let b = crop(&s, 260, 40, 400, 300);
    let blurred = b.resized(50, 37).resized(400, 300);
    let pieces = [piece(a), piece(blurred)];
    let placed = [Some(Affine::IDENTITY), Some(shift(260.0, 40.0))];
    let seam = seams(&pieces, &warps(&placed), &[], &[true, true], &[1.0, 1.0])[0];
    assert!(seam.sharpness < 0.7, "{seam:?}");
    assert_ne!(seam.verdict(), Verdict::Clean);
}

#[test]
fn a_mosaic_anchored_on_a_tilted_shot_is_turned_upright() {
    let turn = |deg: f64| {
        let (s, c) = deg.to_radians().sin_cos();
        Affine {
            a: c,
            b: -s,
            c: 0.0,
            d: s,
            e: c,
            f: 0.0,
        }
    };
    let link = |a, b, map, correlation| Link {
        a,
        b,
        map,
        correlation,
        overlap: 0.3,
        kind: LinkKind::Overlap,
        points: Vec::new(),
    };
    // 2 だけが 3° 傾いて撮れていて、いちばん多くの組を持つので基準になる。
    let links = [
        link(0, 2, turn(3.0), 0.9),
        link(1, 2, shift(-300.0, 0.0).then(&turn(3.0)), 0.9),
        link(0, 1, shift(-300.0, 0.0), 0.5),
    ];
    let placed = tree(3, &links);
    let angle = |i: usize| {
        let (p, _) = placed[i].unwrap();
        p.d.atan2(p.a).to_degrees()
    };
    assert!(angle(0).abs() < 1e-6 && angle(1).abs() < 1e-6, "{placed:?}");
    assert!((angle(2).abs() - 3.0).abs() < 1e-6, "{placed:?}");
}

#[test]
fn a_darker_but_equally_sharp_tile_is_not_called_blurred() {
    let s = scene(500, 300);
    let a = crop(&s, 0, 0, 300, 300);
    let mut b = crop(&s, 200, 0, 300, 300);
    for v in &mut b.data {
        *v *= 0.75;
    }
    let pieces = [piece(a), piece(b)];
    let placed = [Some(Affine::IDENTITY), Some(shift(200.0, 0.0))];
    let seam = seams(&pieces, &warps(&placed), &[], &[true, true], &[1.0, 1.0])[0];
    assert!(seam.sharpness > 0.9, "{seam:?}");
    assert!((seam.brightness - 0.25).abs() < 0.02, "{seam:?}");
}

#[test]
fn the_brightness_left_after_matching_is_what_the_seam_reports() {
    let s = scene(500, 300);
    let a = crop(&s, 0, 0, 300, 300);
    let mut b = crop(&s, 200, 0, 300, 300);
    for v in &mut b.data {
        *v *= 0.8;
    }
    let pieces = [piece(a), piece(b)];
    let placed = [Some(Affine::IDENTITY), Some(shift(200.0, 0.0))];
    let seam = seams(&pieces, &warps(&placed), &[], &[true, true], &[1.0, 1.25])[0];
    assert!(seam.brightness < 0.01, "{seam:?}");
}
