//! 小さな行列の計算（貼り合わせの写し方を求めるのに使う）。

/// 3 × 3 の行列（行ごと）。
pub type Mat3 = [[f64; 3]; 3];

pub const IDENTITY3: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub fn mul3(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

pub fn transpose3(m: &Mat3) -> Mat3 {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = m[j][i];
        }
    }
    out
}

pub fn apply3(m: &Mat3, v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

pub fn invert3(m: &Mat3) -> Option<Mat3> {
    let c = |r: usize, s: usize| {
        let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
        let (s1, s2) = ((s + 1) % 3, (s + 2) % 3);
        m[r1][s1] * m[r2][s2] - m[r1][s2] * m[r2][s1]
    };
    let det = m[0][0] * c(0, 0) + m[0][1] * c(0, 1) + m[0][2] * c(0, 2);
    if det.abs() < 1e-300 || !det.is_finite() {
        return None;
    }
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            // 余因子行列の転置。
            *v = c(j, i) / det;
        }
    }
    Some(out)
}

pub fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn normalized3(v: [f64; 3]) -> Option<[f64; 3]> {
    let n = dot3(v, v).sqrt();
    (n > 1e-300 && n.is_finite()).then(|| v.map(|x| x / n))
}

/// 回転ベクトル（向きが軸、長さが角度）の回転行列（Rodrigues の式）。
pub fn rotation_of(w: [f64; 3]) -> Mat3 {
    let angle = dot3(w, w).sqrt();
    if angle < 1e-15 {
        return IDENTITY3;
    }
    let [x, y, z] = w.map(|v| v / angle);
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    [
        [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
        [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
        [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
    ]
}

/// 連立 1 次方程式（ガウスの消去法、部分ピボット）。解けなければ `None`。
pub fn solve(mut h: Vec<Vec<f64>>, mut g: Vec<f64>) -> Option<Vec<f64>> {
    let n = g.len();
    for col in 0..n {
        let pivot = (col..n).max_by(|&x, &y| h[x][col].abs().total_cmp(&h[y][col].abs()))?;
        if h[pivot][col].abs() < 1e-12 {
            return None;
        }
        h.swap(col, pivot);
        g.swap(col, pivot);
        let (top, below) = h.split_at_mut(col + 1);
        let pivot_row = &top[col];
        for (row, r) in below.iter_mut().enumerate() {
            let k = r[col] / pivot_row[col];
            if k == 0.0 {
                continue;
            }
            for (v, p) in r[col..].iter_mut().zip(&pivot_row[col..]) {
                *v -= k * p;
            }
            g[col + 1 + row] -= k * g[col];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let s: f64 = (row + 1..n).map(|c| h[row][c] * x[c]).sum();
        x[row] = (g[row] - s) / h[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// 対称行列の固有値と固有ベクトル（Jacobi の方法）。固有値の小さい順に、(固有値, 長さ 1 の固有ベクトル)。
pub fn symmetric_eigen(mut a: Vec<Vec<f64>>) -> Vec<(f64, Vec<f64>)> {
    let n = a.len();
    let mut v: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| f64::from(u8::from(i == j))).collect())
        .collect();
    for _ in 0..100 {
        let off: f64 = (0..n)
            .flat_map(|i| (0..n).filter(move |&j| j != i).map(move |j| (i, j)))
            .map(|(i, j)| a[i][j] * a[i][j])
            .sum();
        if off < 1e-24 {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                if a[p][q].abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for row in a.iter_mut() {
                    let (akp, akq) = (row[p], row[q]);
                    row[p] = c * akp - s * akq;
                    row[q] = s * akp + c * akq;
                }
                let (top, bottom) = a.split_at_mut(q);
                for (apk, aqk) in top[p].iter_mut().zip(bottom[0].iter_mut()) {
                    let (x, y) = (*apk, *aqk);
                    *apk = c * x - s * y;
                    *aqk = s * x + c * y;
                }
                for row in v.iter_mut() {
                    let (vp, vq) = (row[p], row[q]);
                    row[p] = c * vp - s * vq;
                    row[q] = s * vp + c * vq;
                }
            }
        }
    }
    let mut out: Vec<(f64, Vec<f64>)> = (0..n)
        .map(|i| (a[i][i], (0..n).map(|k| v[k][i]).collect()))
        .collect();
    out.sort_by(|x, y| x.0.total_cmp(&y.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_system_is_solved() {
        let x = solve(vec![vec![2.0, 1.0], vec![1.0, 3.0]], vec![3.0, 5.0]).unwrap();
        assert!((x[0] - 0.8).abs() < 1e-12 && (x[1] - 1.4).abs() < 1e-12);
    }

    #[test]
    fn eigenvectors_of_a_symmetric_matrix_are_found_smallest_first() {
        let e = symmetric_eigen(vec![
            vec![2.0, 1.0, 0.0],
            vec![1.0, 2.0, 0.0],
            vec![0.0, 0.0, 5.0],
        ]);
        let values: Vec<f64> = e.iter().map(|(v, _)| *v).collect();
        assert!((values[0] - 1.0).abs() < 1e-9);
        assert!((values[1] - 3.0).abs() < 1e-9);
        assert!((values[2] - 5.0).abs() < 1e-9);
        let v = &e[0].1;
        assert!((v[0] + v[1]).abs() < 1e-9 && v[2].abs() < 1e-9);
    }

    #[test]
    fn a_rotation_and_its_transpose_cancel() {
        let r = rotation_of([0.1, -0.4, 0.25]);
        let i = mul3(&r, &transpose3(&r));
        for (a, row) in i.iter().enumerate() {
            for (b, v) in row.iter().enumerate() {
                assert!((v - f64::from(u8::from(a == b))).abs() < 1e-12);
            }
        }
        let back = invert3(&r).unwrap();
        assert!((back[0][1] - r[1][0]).abs() < 1e-12);
    }
}
