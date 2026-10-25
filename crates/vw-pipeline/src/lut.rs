//! パイプラインの色の部分を、標準の形式の 3D LUT（CLF と .cube）に書き出す。
//!
//! 3D LUT は「色 → 色」の表なので、画素ごとに決まる処理（レベル、反転、混合、トーンなど）しか
//! 入らない。周りの画素を見るノード（ぼかし、ノイズ除去など）と、伸縮より前のノード（線形の値に
//! かかる）は入れられないので、そのときは書き出さずに理由を返す。

use crate::nodes::{self, Float, Node};

/// 1 辺の格子の数。.cube と CLF でよく使われる大きさで、なめらかなトーンにも足りる。
pub const GRID: usize = 33;

/// `chain` を表示の値（RGB、0〜1）の格子にかけた表。並びは赤がいちばん速く変わる（.cube の順）。
pub fn sample(chain: &[Node]) -> Result<Vec<[f32; 3]>, String> {
    let after = match nodes::split_at_stretch(chain).map_err(|(_, e)| e)? {
        None => chain,
        Some(s) if s.before.is_empty() => s.after,
        Some(_) => {
            return Err(
                "Nodes before the stretch node work on linear values and cannot go into a LUT."
                    .into(),
            )
        }
    };
    if after.iter().any(Node::spatial) {
        return Err("Nodes that look at neighbouring pixels cannot go into a LUT.".into());
    }
    let step = 1.0 / (GRID - 1) as f32;
    let mut data = Vec::with_capacity(GRID * GRID * GRID * 3);
    for b in 0..GRID {
        for g in 0..GRID {
            for r in 0..GRID {
                data.extend([r as f32 * step, g as f32 * step, b as f32 * step]);
            }
        }
    }
    // 位置で値の変わる式（x・y を使うもの）は表にできない。格子を 2 通りの並びで計算して比べる。
    let mut img = Float::new(GRID * GRID * GRID, 1, 3, data.clone());
    nodes::apply(after, &mut img).map_err(|(_, e)| e)?;
    let mut column = Float::new(1, GRID * GRID * GRID, 3, data);
    nodes::apply(after, &mut column).map_err(|(_, e)| e)?;
    if img
        .data
        .iter()
        .zip(&column.data)
        .any(|(a, b)| (a - b).abs() > 1e-5)
    {
        return Err("An expression uses the pixel position and cannot go into a LUT.".into());
    }
    Ok(img
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| c.map(|v| v.clamp(0.0, 1.0)))
        .collect())
}

/// Adobe / Resolve の .cube。
pub fn to_cube(title: &str, table: &[[f32; 3]]) -> String {
    let mut out = format!(
        "TITLE \"{}\"\nLUT_3D_SIZE {GRID}\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1\n",
        title.replace('"', "'")
    );
    for [r, g, b] in table {
        out.push_str(&format!("{r:.6} {g:.6} {b:.6}\n"));
    }
    out
}

/// Academy / ASC の Common LUT Format（CLF 3.0）。OpenColorIO 2 でも読める。
/// CLF の LUT3D は青がいちばん速く変わる並びなので、.cube の順から並べ替える。
pub fn to_clf(title: &str, table: &[[f32; 3]]) -> String {
    let text = escape(title);
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <ProcessList id=\"vw\" name=\"{text}\" compCLFversion=\"3.0\">\n\
         \x20   <Description>{text} (written by vw)</Description>\n\
         \x20   <LUT3D id=\"look\" inBitDepth=\"32f\" outBitDepth=\"32f\" interpolation=\"tetrahedral\">\n\
         \x20       <Array dim=\"{GRID} {GRID} {GRID} 3\">\n"
    );
    for r in 0..GRID {
        for g in 0..GRID {
            for b in 0..GRID {
                let [x, y, z] = table[(b * GRID + g) * GRID + r];
                out.push_str(&format!("{x:.6} {y:.6} {z:.6}\n"));
            }
        }
    }
    out.push_str("        </Array>\n    </LUT3D>\n</ProcessList>\n");
    out
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invert_becomes_a_lut_with_red_changing_fastest_in_cube() {
        let table = sample(&[Node::Invert]).unwrap();
        assert_eq!(table.len(), GRID * GRID * GRID);
        assert_eq!(table[0], [1.0, 1.0, 1.0]);
        assert_eq!(table[1][0], 1.0 - 1.0 / (GRID - 1) as f32);
        let cube = to_cube("inv", &table);
        let lines: Vec<&str> = cube.lines().collect();
        assert_eq!(lines[1], "LUT_3D_SIZE 33");
        assert_eq!(lines[4], "1.000000 1.000000 1.000000");
        assert_eq!(lines[5], "0.968750 1.000000 1.000000");
    }

    #[test]
    fn clf_lists_blue_fastest() {
        let table = sample(&[]).unwrap();
        let clf = to_clf("a & b", &table);
        assert!(clf.contains("name=\"a &amp; b\""));
        let rows: Vec<&str> = clf
            .lines()
            .filter(|l| l.split(' ').count() == 3 && !l.contains('<'))
            .collect();
        assert_eq!(rows[1], "0.000000 0.000000 0.031250");
    }

    #[test]
    fn neighbourhood_and_linear_nodes_are_refused() {
        assert!(sample(&[Node::Blur { radius: 1.0 }]).is_err());
        assert!(sample(&[Node::Invert, Node::Stretch]).is_err());
        assert!(sample(&[Node::Stretch, Node::Invert]).is_ok());
        let position = Node::Expression {
            text: "v * x".into(),
        };
        assert!(sample(&[position]).is_err());
    }
}
