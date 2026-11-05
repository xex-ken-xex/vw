use super::*;
use crate::test_util::{TempDir, png_registry, write_png};
use vw_core::DecoderPlugin as _;

fn quality(sharpness: Option<f64>, fwhm: Option<f32>) -> Quality {
    Quality { sharpness, fwhm }
}

fn prepared(quality: Quality) -> Prepared {
    Prepared {
        path: PathBuf::new(),
        thumb: Rgb::from_rgba8(1, 1, &[0, 0, 0, 255]),
        quality,
        wcs: None,
        color: false,
        depth: Depth::U8,
        window: None,
        curve: Curve::default(),
        fits: false,
        measured: None,
        stars: None,
        focal: None,
    }
}

#[test]
fn the_sharpest_photo_of_an_area_is_chosen_first() {
    let images = [
        prepared(quality(Some(3.0), None)),
        prepared(quality(Some(9.0), None)),
        prepared(quality(Some(5.0), None)),
    ];
    assert_eq!(best_of(&images, &[0, 1, 2]), 1);
}

#[test]
fn the_astro_frame_with_the_thinnest_stars_is_chosen_first() {
    let images = [
        prepared(quality(Some(9.0), Some(3.1))),
        prepared(quality(Some(2.0), Some(2.4))),
    ];
    assert_eq!(best_of(&images, &[0, 1]), 1);
}

#[test]
fn mixed_bit_depths_are_stitched_as_real_numbers() {
    assert_eq!(depth_of(&[Depth::U8, Depth::U8]), Depth::U8);
    assert_eq!(depth_of(&[Depth::U16(12), Depth::U16(14)]), Depth::U16(14));
    assert_eq!(depth_of(&[Depth::U8, Depth::U16(16)]), Depth::F32);
    assert_eq!(depth_of(&[Depth::U16(16), Depth::F32]), Depth::F32);
}

#[test]
fn a_point_is_inside_the_outline_of_a_turned_image() {
    let diamond = [
        pos2(10.0, 0.0),
        pos2(20.0, 10.0),
        pos2(10.0, 20.0),
        pos2(0.0, 10.0),
    ];
    assert!(inside_polygon(pos2(10.0, 10.0), &diamond));
    assert!(!inside_polygon(pos2(1.0, 1.0), &diamond));
}

/// 全体に散らした、ぼけた点の模様の RGB（どこを切り出しても似た所のない絵）。
fn scene(width: usize, height: usize) -> Vec<u8> {
    let mut seed = 0x2545_f491_u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        f64::from(seed) / f64::from(u32::MAX)
    };
    let mut v = vec![110.0f64; width * height];
    for _ in 0..width * height / 900 {
        let (bx, by, r, amp) = (
            next() * width as f64,
            next() * height as f64,
            2.0 + next() * 9.0,
            next() * 2.0 - 1.0,
        );
        let reach = (r * 3.0) as isize;
        for dy in -reach..=reach {
            for dx in -reach..=reach {
                let (x, y) = (bx as isize + dx, by as isize + dy);
                if x < 0 || y < 0 || x >= width as isize || y >= height as isize {
                    continue;
                }
                let d2 = (x as f64 - bx).powi(2) + (y as f64 - by).powi(2);
                v[y as usize * width + x as usize] += amp * 100.0 * (-d2 / (2.0 * r * r)).exp();
            }
        }
    }
    v.iter()
        .flat_map(|&g| {
            let g = g.clamp(0.0, 255.0) as u8;
            [g, g, g]
        })
        .collect()
}

fn crop(s: &[u8], sw: usize, dir: &Path, name: &str, at: [usize; 2], size: [usize; 2]) -> PathBuf {
    let rgb: Vec<u8> = (0..size[0] * size[1])
        .flat_map(|i| {
            let p = ((at[1] + i / size[0]) * sw + at[0] + i % size[0]) * 3;
            [s[p], s[p + 1], s[p + 2]]
        })
        .collect();
    let path = dir.join(name);
    write_png(&path, size[0] as u32, size[1] as u32, &rgb);
    path
}

fn wait(job: Job) -> Found {
    let mut ready: Vec<Option<(Prepared, Piece)>> = Vec::new();
    loop {
        match job.rx.recv().expect("the job finishes") {
            Message::Total(n) => ready = (0..n).map(|_| None).collect(),
            Message::Ready(i, p) => ready[i] = Some(*p),
            Message::Problem(e) => panic!("{e}"),
            Message::Linking(_) => {}
            Message::Done(links, features) => {
                return Found::new(
                    ready.into_iter().flatten().collect(),
                    links,
                    features,
                    StitchFeatures::Fallback,
                );
            }
        }
    }
}

#[test]
fn three_overlapping_photos_are_found_placed_and_planned() {
    let tmp = TempDir::new();
    let s = scene(700, 400);
    let at = [[0, 0], [260, 40], [290, 60]];
    let paths: Vec<PathBuf> = ["a.png", "b.png", "c.png"]
        .iter()
        .zip(at)
        .map(|(name, at)| crop(&s, 700, tmp.path(), name, at, [400, 300]))
        .collect();
    let found = wait(start_job(
        paths.clone(),
        png_registry(),
        StitchFeatures::Fallback,
    ));
    assert_eq!(found.images.len(), 3);
    let settings = StitchSettings::default();
    let arranged = arrange(
        &found,
        &found.links,
        &HashMap::new(),
        StitchProjection::Auto,
        &settings,
    );
    // b と c は 8 割以上（86 %）重なるので、同じ所を写した 1 組になり、どちらか 1 枚だけを使う。
    assert_eq!(arranged.groups, vec![vec![1, 2]]);
    assert_eq!(arranged.used.iter().filter(|u| **u).count(), 2);
    let plan = plan_of(&found, &found.links, &arranged, &settings).unwrap();
    assert_eq!(plan.sources.len(), 2);
    let a = plan.sources.iter().find(|s| s.path == paths[0]).unwrap();
    let other = plan.sources.iter().find(|s| s.path != paths[0]).unwrap();
    let i = paths.iter().position(|p| *p == other.path).unwrap();
    let want = [
        at[i][0] as f64 - at[0][0] as f64,
        at[i][1] as f64 - at[0][1] as f64,
    ];
    let a_origin = a.placed.apply([0.0, 0.0]).unwrap();
    let got = other.placed.apply([0.0, 0.0]).unwrap();
    let offset = [got[0] - a_origin[0], got[1] - a_origin[1]];
    assert!(
        (offset[0] - want[0]).hypot(offset[1] - want[1]) < 0.5,
        "{offset:?} {want:?}"
    );
    assert_eq!(other.how, How::Matched);
    assert!(other.correlation.unwrap() > 0.9);
    assert!(arranged.preview.is_some());
}

#[test]
fn stitching_the_same_photos_twice_gives_the_same_placement_and_reference() {
    let tmp = TempDir::new();
    let s = scene(700, 400);
    let at = [[0, 0], [260, 40], [290, 60]];
    let paths: Vec<PathBuf> = ["a.png", "b.png", "c.png"]
        .iter()
        .zip(at)
        .map(|(name, at)| crop(&s, 700, tmp.path(), name, at, [400, 300]))
        .collect();
    let settings = StitchSettings::default();
    let stitch = || {
        let found = wait(start_job(
            paths.clone(),
            png_registry(),
            StitchFeatures::Fallback,
        ));
        let arranged = arrange(
            &found,
            &found.links,
            &HashMap::new(),
            StitchProjection::Auto,
            &settings,
        );
        plan_of(&found, &found.links, &arranged, &settings).unwrap()
    };
    let first = stitch();
    let second = stitch();
    assert_eq!(first, second);
    let reference = first
        .sources
        .iter()
        .find(|s| s.how == How::Reference)
        .unwrap();
    assert_eq!(reference.path, paths[0]);
}

#[test]
fn choosing_another_shot_of_the_same_area_uses_it_instead() {
    let tmp = TempDir::new();
    let s = scene(700, 400);
    let at = [[0, 0], [260, 40], [290, 60]];
    let paths: Vec<PathBuf> = ["a.png", "b.png", "c.png"]
        .iter()
        .zip(at)
        .map(|(name, at)| crop(&s, 700, tmp.path(), name, at, [400, 300]))
        .collect();
    let found = wait(start_job(paths, png_registry(), StitchFeatures::Fallback));
    let settings = StitchSettings::default();
    let first = arrange(
        &found,
        &found.links,
        &HashMap::new(),
        StitchProjection::Auto,
        &settings,
    );
    let chosen = if first.used[1] { 2 } else { 1 };
    let choices = HashMap::from([(1, chosen)]);
    let again = arrange(
        &found,
        &found.links,
        &choices,
        StitchProjection::Auto,
        &settings,
    );
    assert!(again.used[chosen]);
    assert!(!again.used[3 - chosen]);
}

#[test]
fn a_saved_png_does_not_overwrite_an_existing_file() {
    let tmp = TempDir::new();
    let s = scene(700, 400);
    let a = crop(&s, 700, tmp.path(), "a.png", [0, 0], [400, 300]);
    let b = crop(&s, 700, tmp.path(), "b.png", [260, 40], [400, 300]);
    let found = wait(start_job(
        vec![a, b],
        png_registry(),
        StitchFeatures::Fallback,
    ));
    let settings = StitchSettings::default();
    let arranged = arrange(
        &found,
        &found.links,
        &HashMap::new(),
        StitchProjection::Auto,
        &settings,
    );
    let plan = plan_of(&found, &found.links, &arranged, &settings).unwrap();
    let out = tmp.path().join("out.png");
    let note = save(&plan, &png_registry(), &out).unwrap();
    assert!(note.contains("660 x 34"), "{note}");
    let back = vw_png::PngPlugin
        .decode(&std::fs::read(&out).unwrap())
        .unwrap();
    // 1 画素より細かいずれで、高さは 340 か 341 になる。
    assert!(back.width == 660 && (340..=341).contains(&back.height));
    let text = back.metadata.get("Text", "Description").unwrap();
    assert!(text.contains("a.png: x 0, y 0"), "{text}");
    let err = save(&plan, &png_registry(), &out).unwrap_err();
    assert!(err.contains("not overwritten"), "{err}");
}

fn tan_cards(crpix: [f64; 2]) -> Vec<(String, String)> {
    [
        ("NAXIS1", "800".to_owned()),
        ("NAXIS2", "600".to_owned()),
        ("CTYPE1", "RA---TAN".to_owned()),
        ("CTYPE2", "DEC--TAN".to_owned()),
        ("CRPIX1", crpix[0].to_string()),
        ("CRPIX2", crpix[1].to_string()),
        ("CRVAL1", "83.82".to_owned()),
        ("CRVAL2", "-5.39".to_owned()),
        ("CD1_1", "-0.000556".to_owned()),
        ("CD1_2", "0".to_owned()),
        ("CD2_1", "0".to_owned()),
        ("CD2_2", "0.000556".to_owned()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect()
}

#[test]
fn tiles_with_a_wcs_are_placed_by_it_instead_of_by_the_picture() {
    let piece = || Piece {
        plane: vw_compare::plane::Plane::new(8, 6, vec![0.0; 48]),
        full: [800, 600],
    };
    let to_fits = Affine {
        c: 1.0,
        f: 1.0,
        ..Affine::IDENTITY
    };
    // b は a の 600 画素右を写している（参照の画素が 600 左にある）。
    let wcs = [
        Some((tan_cards([1000.5, 525.5]), to_fits)),
        Some((tan_cards([400.5, 525.5]), to_fits)),
        None,
    ];
    let mut links = vec![Link::by_map(
        (0, [800, 600]),
        (1, [800, 600]),
        Affine::IDENTITY,
        0.7,
        0.25,
        LinkKind::Overlap,
    )];
    add_wcs_links(&[piece(), piece(), piece()], &wcs, &mut links);
    assert_eq!(links.len(), 1, "the picture's link is replaced: {links:?}");
    assert_eq!(links[0].kind, LinkKind::Wcs);
    let [x, y] = links[0].map.apply([700.0, 300.0]);
    assert!(
        (x - 100.0).abs() < 0.01 && (y - 300.0).abs() < 0.01,
        "{links:?}"
    );
    assert_eq!(how_of(Some(0), &links), How::Wcs);
}

/// 球に散らしたぼけた点の模様を、正距円筒の絵（横 `w`、縦 `w / 2`）にしたもの。
fn sky_sphere(w: usize) -> Vec<f32> {
    let h = w / 2;
    let mut seed = 0x6a09_e667_u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        f64::from(seed) / f64::from(u32::MAX)
    };
    let mut v = vec![110.0f32; w * h];
    for _ in 0..w * h / 250 {
        let (bx, by, r, amp) = (
            next() * w as f64,
            next() * h as f64,
            1.5 + next() * 4.0,
            (next() * 2.0 - 1.0) * 110.0,
        );
        let reach = (r * 3.0) as isize;
        for dy in -reach..=reach {
            for dx in -reach..=reach {
                let (x, y) = ((bx as isize + dx).rem_euclid(w as isize), by as isize + dy);
                if y < 0 || y >= h as isize {
                    continue;
                }
                let d2 = (dx as f64).powi(2) + (dy as f64).powi(2);
                v[y as usize * w + x as usize] += (amp * (-d2 / (2.0 * r * r)).exp()) as f32;
            }
        }
    }
    v
}

/// その場で右へ `yaw` ラジアン回したカメラ（焦点距離 `f` 画素）で `sphere` を撮った絵（RGB）。
fn shot(sphere: &[f32], sw: usize, yaw: f64, f: f64, size: [usize; 2]) -> Vec<u8> {
    let sh = sw / 2;
    let (cx, cy) = ((size[0] as f64 - 1.0) / 2.0, (size[1] as f64 - 1.0) / 2.0);
    (0..size[0] * size[1])
        .flat_map(|i| {
            let (x, y) = ((i % size[0]) as f64, (i / size[0]) as f64);
            let (dx, dy, dz) = ((x - cx) / f, (y - cy) / f, 1.0);
            // カメラの向きを右へ yaw 回す。
            let (s, c) = yaw.sin_cos();
            let (wx, wz) = (c * dx + s * dz, -s * dx + c * dz);
            let theta = wx.atan2(wz);
            let phi = dy.atan2(wx.hypot(wz));
            let u = (theta / std::f64::consts::TAU + 0.5) * sw as f64;
            let v = (phi / std::f64::consts::PI + 0.5) * sh as f64;
            let (u0, v0) = (u.floor() as isize, v.floor() as isize);
            let (fu, fv) = ((u - u0 as f64) as f32, (v - v0 as f64) as f32);
            let at = |x: isize, y: isize| {
                sphere[(y.clamp(0, sh as isize - 1) as usize) * sw
                    + x.rem_euclid(sw as isize) as usize]
            };
            let g = (at(u0, v0) * (1.0 - fu) + at(u0 + 1, v0) * fu) * (1.0 - fv)
                + (at(u0, v0 + 1) * (1.0 - fu) + at(u0 + 1, v0 + 1) * fu) * fv;
            let g = g.clamp(0.0, 255.0) as u8;
            [g, g, g]
        })
        .collect()
}

#[test]
fn a_panorama_turned_through_two_hundred_degrees_is_put_on_a_cylinder() {
    let tmp = TempDir::new();
    let sphere = sky_sphere(4096);
    let size = [480, 360];
    let paths: Vec<PathBuf> = (0..5)
        .map(|k| {
            let rgb = shot(&sphere, 4096, k as f64 * 0.6, 400.0, size);
            let path = tmp.path().join(format!("p{k}.png"));
            write_png(&path, size[0] as u32, size[1] as u32, &rgb);
            path
        })
        .collect();
    let found = wait(start_job(paths, png_registry(), StitchFeatures::Fallback));
    let settings = StitchSettings::default();
    let arranged = arrange(
        &found,
        &found.links,
        &HashMap::new(),
        StitchProjection::Auto,
        &settings,
    );
    assert_eq!(
        arranged.projection,
        Projection::Cylinder,
        "{:?}",
        arranged.note
    );
    assert!(arranged.placed.iter().all(Option::is_some));
    let (_, after) = arranged.fit.unwrap();
    assert!(after < 1.5, "{:?}", arranged.fit);
    assert!(
        arranged
            .seams
            .iter()
            .all(|s| s.verdict() != Verdict::Misaligned),
        "{:?}",
        arranged.seams
    );
    let plan = plan_of(&found, &found.links, &arranged, &settings).unwrap();
    let grid = plan.grid().unwrap();
    // 横に約 200°、縦は 1 枚の画角ほど。
    assert!(
        grid.width > 3 * grid.height,
        "{} x {}",
        grid.width,
        grid.height
    );
    let text = stitch::placement_text(&plan.sources[0], &grid);
    assert!(text.contains("heading") && text.contains("f "), "{text}");
}

#[test]
fn a_photo_turned_too_far_for_the_overlap_search_is_found_by_its_feature_points() {
    let tmp = TempDir::new();
    // 細かい模様の絵（1400 × 700 の左上 700 × 500 を使う）。
    let texture = sky_sphere(1400);
    let gray =
        |x: usize, y: usize| texture[y.min(699) * 1400 + x.min(1399)].clamp(0.0, 255.0) as u8;
    let cut = |at: [f64; 2], angle: f64| -> Vec<u8> {
        let (sn, cs) = angle.to_radians().sin_cos();
        (0..400 * 300)
            .flat_map(|i| {
                let (x, y) = ((i % 400) as f64 - 199.5, (i / 400) as f64 - 149.5);
                let (u, v) = (cs * x - sn * y + at[0], sn * x + cs * y + at[1]);
                let g = gray(u.round().max(0.0) as usize, v.round().max(0.0) as usize);
                [g, g, g]
            })
            .collect()
    };
    let a = tmp.path().join("a.png");
    write_png(&a, 400, 300, &cut([199.5, 149.5], 0.0));
    // b は真ん中が (450, 300) の所を、35° 回して切り出した。
    let b = tmp.path().join("b.png");
    write_png(&b, 400, 300, &cut([450.0, 300.0], 35.0));
    let found = wait(start_job(
        vec![a, b],
        png_registry(),
        StitchFeatures::Fallback,
    ));
    assert_eq!(found.links.len(), 1, "{:?}", found.links);
    let settings = StitchSettings::default();
    let arranged = arrange(
        &found,
        &found.links,
        &HashMap::new(),
        StitchProjection::Auto,
        &settings,
    );
    let view = arranged_view(&arranged, &found.links);
    assert!(view.how.contains(&Some(How::Features)), "{:?}", view.how);
    let plan = plan_of(&found, &found.links, &arranged, &settings).unwrap();
    let turned = plan
        .sources
        .iter()
        .find(|s| s.how == How::Features)
        .unwrap();
    let grid = plan.grid().unwrap();
    let text = stitch::placement_text(turned, &grid);
    let angle: f64 = text
        .split(", ")
        .find_map(|part| part.strip_suffix('°'))
        .and_then(|v| v.parse().ok())
        .expect("an angle");
    assert!((angle.abs() - 35.0).abs() < 0.1, "{text}");
}
