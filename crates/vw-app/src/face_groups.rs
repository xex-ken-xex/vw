//! 名前のない顔を、似たものどうしにまとめる。「人物」ウインドウの「未登録の顔」に出し、
//! まとめごとに名前を付けて登録できるようにするため。

use crate::faces::{cosine, normalized};

/// 顔の特徴（長さ 1）の並びを、似たものどうしのまとまりに分ける。
/// 戻り値は `features` の番号のまとまりで、顔の多いまとまりから順に並ぶ。
///
/// まとまりの平均との類似度で振り分ける。顔どうしの一番近いものでつなぐやり方
/// （最短距離法）は、少しずつ似た顔を伝って別人まで 1 つにつながってしまうので採らない。
/// 振り分けは顔の順に左右されるので、平均を決め直してもう一度振り分ける。
pub fn group(features: &[&[f32]], threshold: f32) -> Vec<Vec<usize>> {
    let mut centers: Vec<Vec<f32>> = Vec::new();
    for f in features {
        match nearest(&centers, f, threshold) {
            Some(c) => centers[c] = normalized(add(&centers[c], f)),
            None => centers.push(f.to_vec()),
        }
    }
    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); centers.len()];
    let mut alone = Vec::new();
    for (i, f) in features.iter().enumerate() {
        match nearest(&centers, f, threshold) {
            Some(c) => groups[c].push(i),
            None => alone.push(vec![i]),
        }
    }
    groups.retain(|g| !g.is_empty());
    groups.extend(alone);
    groups.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));
    groups
}

fn nearest(centers: &[Vec<f32>], f: &[f32], threshold: f32) -> Option<usize> {
    centers
        .iter()
        .enumerate()
        .map(|(i, c)| (i, cosine(c, f)))
        .filter(|(_, s)| *s >= threshold)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

fn add(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b).map(|(x, y)| x + y).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(v: [f32; 3]) -> Vec<f32> {
        normalized(v.to_vec())
    }

    #[test]
    fn similar_faces_form_one_group_and_the_largest_comes_first() {
        let faces = [
            face([1.0, 0.0, 0.0]),
            face([0.0, 1.0, 0.0]),
            face([0.95, 0.1, 0.0]),
            face([0.9, 0.0, 0.1]),
            face([0.1, 0.95, 0.0]),
        ];
        let refs: Vec<&[f32]> = faces.iter().map(Vec::as_slice).collect();
        assert_eq!(group(&refs, 0.36), [vec![0, 2, 3], vec![1, 4]]);
    }

    #[test]
    fn a_face_like_no_other_stays_alone() {
        let faces = [
            face([1.0, 0.0, 0.0]),
            face([0.0, 0.0, 1.0]),
            face([0.95, 0.1, 0.0]),
        ];
        let refs: Vec<&[f32]> = faces.iter().map(Vec::as_slice).collect();
        assert_eq!(group(&refs, 0.36), [vec![0, 2], vec![1]]);
    }

    #[test]
    fn faces_that_only_chain_through_each_other_are_not_merged() {
        // 隣どうしは似ているが、両端は似ていない並び。
        let faces: Vec<Vec<f32>> = (0..7)
            .map(|i| {
                let a = i as f32 * 0.35;
                face([a.cos(), a.sin(), 0.0])
            })
            .collect();
        let refs: Vec<&[f32]> = faces.iter().map(Vec::as_slice).collect();
        let groups = group(&refs, 0.8);
        assert!(groups.len() > 1, "{groups:?}");
        for g in &groups {
            for &i in g {
                for &j in g {
                    assert!(cosine(&faces[i], &faces[j]) > 0.3, "{groups:?}");
                }
            }
        }
    }

    #[test]
    fn no_faces_make_no_groups() {
        assert!(group(&[], 0.36).is_empty());
    }
}
