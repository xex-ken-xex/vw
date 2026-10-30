use super::*;

/// 星の位置（決まった並び）。
fn star_positions() -> Vec<[f64; 2]> {
    let mut state = 11u64;
    let mut next = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as f64 / (1u64 << 31) as f64
    };
    (0..40)
        .map(|_| [20.0 + next() * 200.0, 20.0 + next() * 160.0])
        .collect()
}

/// 背景 100、雑音 σ 1 の空に、`stars` の位置に σ `sx`×`sy` の星を描く。
fn sky(stars: &[[f64; 2]], sx: f32, sy: f32, background: f32) -> Float {
    let (w, h) = (240usize, 200usize);
    let mut state = 5u64;
    let mut data: Vec<f32> = (0..w * h)
        .map(|_| background + crate::nodes::gaussian(&mut state))
        .collect();
    for (n, s) in stars.iter().enumerate() {
        let peak = 200.0 + 40.0 * n as f32;
        for y in 0..h {
            for x in 0..w {
                let dx = (x as f64 - s[0]) as f32 / sx;
                let dy = (y as f64 - s[1]) as f32 / sy;
                let r2 = dx * dx + dy * dy;
                if r2 < 50.0 {
                    data[y * w + x] += peak * (-r2 / 2.0).exp();
                }
            }
        }
    }
    Float::new(w, h, 1, data)
}

#[test]
fn evaluation_measures_fwhm_stars_background_and_noise() {
    let e = evaluate(&sky(&star_positions(), 1.5, 1.5, 100.0));
    let fwhm = e.fwhm.unwrap();
    assert!((fwhm - 1.5 * FWHM_PER_SIGMA).abs() < 0.4, "{fwhm}");
    assert!(e.stars >= 30, "{}", e.stars);
    assert!((e.background - 100.0).abs() < 1.0);
    assert!((e.noise - 1.0).abs() < 0.2, "{}", e.noise);
    assert!(e.eccentricity.unwrap() < 0.45);
}

#[test]
fn trailed_stars_have_a_high_eccentricity() {
    let round = evaluate(&sky(&star_positions(), 1.6, 1.6, 100.0));
    let trailed = evaluate(&sky(&star_positions(), 3.2, 1.6, 100.0));
    assert!(trailed.eccentricity.unwrap() > round.eccentricity.unwrap() + 0.3);
}

#[test]
fn alignment_recovers_a_shift_and_a_rotation() {
    let reference = star_positions();
    let (angle, shift): (f64, [f64; 2]) = (0.05, [6.5, -4.25]);
    let (c, s) = (angle.cos(), angle.sin());
    let center = [120.0, 100.0];
    let moved: Vec<[f64; 2]> = reference
        .iter()
        .map(|p| {
            let (x, y) = (p[0] - center[0], p[1] - center[1]);
            [
                c * x - s * y + center[0] + shift[0],
                s * x + c * y + center[1] + shift[1],
            ]
        })
        .collect();
    let a = evaluate(&sky(&reference, 1.5, 1.5, 100.0));
    let b = evaluate(&sky(&moved, 1.5, 1.5, 100.0));
    let m = align(&a.bright, &b.bright).expect("aligned");
    for (p, q) in reference.iter().zip(&moved).take(10) {
        let got = apply(&m, *p);
        assert!(distance(got, *q) < 0.3, "{got:?} vs {q:?}");
    }
}

#[test]
fn alignment_handles_a_half_turn_after_a_meridian_flip() {
    let reference = star_positions();
    let flipped: Vec<[f64; 2]> = reference
        .iter()
        .map(|p| [239.0 - p[0], 199.0 - p[1]])
        .collect();
    let a = evaluate(&sky(&reference, 1.5, 1.5, 100.0));
    let b = evaluate(&sky(&flipped, 1.5, 1.5, 100.0));
    let m = align(&a.bright, &b.bright).expect("aligned");
    let got = apply(&m, reference[0]);
    assert!(distance(got, flipped[0]) < 0.3, "{got:?}");
}

#[test]
fn unrelated_star_fields_do_not_align() {
    let reference = star_positions();
    let other: Vec<[f64; 2]> = reference
        .iter()
        .map(|p| [(p[0] * 7.3) % 200.0 + 20.0, (p[1] * 3.1) % 160.0 + 20.0])
        .collect();
    let a = evaluate(&sky(&reference, 1.5, 1.5, 100.0));
    let b = evaluate(&sky(&other, 1.5, 1.5, 100.0));
    assert!(align(&a.bright, &b.bright).is_none());
}

#[test]
fn blurred_trailed_cloudy_and_bright_frames_get_their_reasons() {
    let stars = star_positions();
    let good = evaluate(&sky(&stars, 1.5, 1.5, 100.0));
    let blurred = evaluate(&sky(&stars, 2.6, 2.6, 100.0));
    let trailed = evaluate(&sky(&stars, 3.5, 1.5, 100.0));
    let cloudy = evaluate(&sky(&stars[..8], 1.5, 1.5, 100.0));
    let bright = evaluate(&sky(&stars, 1.5, 1.5, 130.0));
    let all = [
        good.clone(),
        good.clone(),
        good,
        blurred,
        trailed,
        cloudy,
        bright,
    ];
    let limits = Limits {
        fwhm_ratio: 1.3,
        eccentricity: 0.7,
        stars_ratio: 0.5,
        background_sigmas: 10.0,
    };
    let r = reasons(&all, limits);
    assert!(
        r[0].is_empty() && r[1].is_empty() && r[2].is_empty(),
        "{r:?}"
    );
    assert!(r[3].contains(&Reason::Blurred), "{r:?}");
    assert!(r[4].contains(&Reason::Elongated), "{r:?}");
    assert!(r[5].contains(&Reason::FewStars), "{r:?}");
    assert_eq!(r[6], vec![Reason::BrightSky]);
}
