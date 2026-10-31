use super::*;
use vw_pipeline::subframes::Point;

fn preview(width: usize, height: usize, scale: f64, pixels: Vec<u8>) -> Preview {
    Preview {
        width,
        height,
        scale,
        pixels,
    }
}

fn row(order: usize, fwhm: Option<f32>, stars: usize) -> Row {
    Row {
        path: PathBuf::from(format!("{order}.fits")),
        order,
        evaluation: Evaluation {
            fwhm,
            eccentricity: None,
            stars,
            background: 0.0,
            noise: 0.0,
            bright: (0..stars)
                .map(|i| Point {
                    x: i as f32,
                    y: 0.0,
                })
                .collect(),
        },
        preview: preview(1, 1, 1.0, vec![0]),
    }
}

#[test]
fn preview_shrinks_the_long_side_and_stretches_the_sky_to_gray() {
    let (w, h) = (300, 100);
    let mut data = vec![100.0f32; w * h];
    for (i, v) in data.iter_mut().enumerate() {
        *v += (i % 7) as f32;
    }
    data[50 * w + 150] = 60000.0;
    let p = preview_of(&Float::new(w, h, 1, data), 100);
    assert_eq!((p.width, p.height), (100, 33));
    assert!((p.scale - 1.0 / 3.0).abs() < 1e-9);
    let mut sky = p.pixels;
    sky.sort_unstable();
    let median = sky[sky.len() / 2];
    assert!((40..=100).contains(&median), "sky {median}");
}

#[test]
fn warp_with_the_identity_keeps_the_image() {
    let p = preview(3, 2, 0.5, vec![0, 50, 100, 150, 200, 250]);
    assert_eq!(warp(&p, &p, &IDENTITY), p.pixels);
}

#[test]
fn warp_moves_the_other_image_onto_the_reference() {
    // 元の解像度で 2 画素右にずれた画像は、縮小画像（半分）では 1 画素右にずれて見える。
    let reference = preview(4, 1, 0.5, vec![0, 0, 0, 0]);
    let other = preview(4, 1, 0.5, vec![0, 0, 255, 0]);
    let shift = [[1.0, 0.0, 2.0], [0.0, 1.0, 0.0]];
    assert_eq!(warp(&reference, &other, &shift), [0, 255, 0, 0]);
}

#[test]
fn outside_the_other_image_is_black() {
    let reference = preview(2, 1, 1.0, vec![0, 0]);
    let other = preview(2, 1, 1.0, vec![200, 200]);
    let shift = [[1.0, 0.0, 1.0], [0.0, 1.0, 0.0]];
    assert_eq!(warp(&reference, &other, &shift), [200, 0]);
}

#[test]
fn blink_leaves_out_unaligned_and_optionally_rejected_frames() {
    let maps = [Some(IDENTITY), None, Some(IDENTITY), Some(IDENTITY)];
    let reasons = vec![vec![], vec![], vec![Reason::Blurred], vec![]];
    assert_eq!(blink_frames(&maps, &reasons, true), [0, 3]);
    assert_eq!(blink_frames(&maps, &reasons, false), [0, 2, 3]);
}

#[test]
fn stepping_wraps_around_at_both_ends() {
    let frames = [0, 3, 5];
    assert_eq!(step(&frames, 5, true), 0);
    assert_eq!(step(&frames, 0, false), 5);
    assert_eq!(step(&frames, 3, true), 5);
    assert_eq!(step(&frames, 4, true), 0);
}

#[test]
fn the_sharpest_frame_with_enough_stars_is_the_reference() {
    let rows = [
        row(0, Some(3.0), 10),
        row(1, Some(1.0), 2),
        row(2, Some(2.0), 10),
    ];
    assert_eq!(reference_of(&rows), Some(2));
}

#[test]
fn sorting_puts_frames_without_a_value_last_either_way() {
    let rows = [row(0, Some(3.0), 5), row(1, None, 0), row(2, Some(2.0), 5)];
    assert_eq!(sorted(&rows, (Column::Fwhm, true)), [2, 0, 1]);
    assert_eq!(sorted(&rows, (Column::Fwhm, false)), [0, 2, 1]);
    assert_eq!(sorted(&rows, (Column::Name, false)), [2, 1, 0]);
}

#[test]
fn small_and_large_values_keep_their_digits() {
    assert_eq!(number(1234.6), "1235");
    assert_eq!(number(12.345), "12.35");
    assert_eq!(number(0.01234), "0.0123");
}
