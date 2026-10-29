//! 1 つのファイルを読み、保存したパイプラインをかけて、書き出す画素にする。
//! 描く処理はビューアと同じ [`vw_pipeline::render`] を使い、色の表もビューアと同じ引き方にする。

use std::path::Path;

use vw_core::{ColorModel, Palette, Registry, VwImage};
use vw_dicom::pipeline::{Gray, Pipeline};
use vw_pipeline::look::Look;
use vw_pipeline::preset::Values;

/// かけるもの。
pub struct Recipe {
    pub values: Values,
    pub look: Look,
    pub palette: Option<Palette>,
    /// カメラ RAW をセンサーのデータから現像する（しなければ埋め込みのプレビュー）。
    pub develop: bool,
    /// 何枚も入ったファイル（アニメーション、シネ）の全部のフレームを書く（しなければ最初の 1 枚）。
    pub all_frames: bool,
}

/// 書き出す画素。値は 0〜1、行ごと・画素ごと・チャンネルごとの順。
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    /// 1 灰色、2 灰色 + アルファ、3 RGB、4 RGBA。
    pub channels: usize,
    pub data: Vec<f32>,
}

impl Picture {
    pub fn has_alpha(&self) -> bool {
        self.channels.is_multiple_of(2)
    }

    /// アルファを除いたもの（アルファのない形式に書くとき）。
    pub fn without_alpha(&self) -> Picture {
        if !self.has_alpha() {
            return self.clone();
        }
        let color = self.channels - 1;
        let data = self
            .data
            .chunks_exact(self.channels)
            .flat_map(|px| px[..color].iter().copied())
            .collect();
        Picture {
            channels: color,
            data,
            ..*self
        }
    }
}

/// `path` の画像（`all_frames` ならフレームごと）。フレームが 1 枚なら番号は `None`。
/// デコーダや現像の panic は、そのファイルのエラーにする。並列に回しているので、
/// 1 枚の panic で全体が止まり、残りのファイルが書かれなくなるため。
pub fn convert(
    registry: &Registry,
    path: &Path,
    recipe: &Recipe,
) -> Result<Vec<(Option<usize>, Picture)>, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        convert_unguarded(registry, path, recipe)
    }))
    .unwrap_or_else(|panic| Err(format!("internal error: {}", panic_text(&*panic))))
}

fn panic_text(panic: &(dyn std::any::Any + Send)) -> &str {
    panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("panic")
}

fn convert_unguarded(
    registry: &Registry,
    path: &Path,
    recipe: &Recipe,
) -> Result<Vec<(Option<usize>, Picture)>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let frames = match recipe.all_frames {
        true => registry
            .frame_count(path, &bytes)
            .map_err(|e| e.to_string())?,
        false => 1,
    };
    (0..frames.max(1))
        .map(|frame| {
            let (mut image, plugin) = registry
                .decode_frame_bytes(path, &bytes, frame)
                .map_err(|e| e.to_string())?;
            if recipe.develop && image.develop_size.is_some() {
                image = registry
                    .develop_bytes(path, &bytes)
                    .map_err(|e| e.to_string())?
                    .0;
            }
            let info = match plugin {
                "DICOM" => match vw_dicom::pipeline::read(&bytes, frame) {
                    Ok(Pipeline::Gray(g)) => Some(g),
                    Ok(Pipeline::Color(_)) => None,
                    Err(e) => return Err(e.to_string()),
                },
                _ => None,
            };
            let picture = picture(&mut image, info.as_deref(), recipe)?;
            Ok(((frames > 1).then_some(frame), picture))
        })
        .collect()
}

/// 1 枚にかける。ビューアのスロットで、見た目を当ててから色導出を描くのと同じ順。
pub fn picture(
    image: &mut VwImage,
    info: Option<&Gray>,
    recipe: &Recipe,
) -> Result<Picture, String> {
    let window = recipe.look.apply(image);
    let r = vw_pipeline::render::render(image, info, &recipe.values, window, None);
    if let Err(m) = r.checked {
        return Err(format!(
            "the pipeline cannot be applied to this image ({} pixels differ from the standard display)",
            m.pixels
        ));
    }
    if let Some((i, e)) = r.error {
        return Err(format!("node {}: {e}", i + 1));
    }
    let float = r.float;
    let alpha = matches!(image.color, ColorModel::GrayAlpha | ColorModel::Rgba);
    let level = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as usize;
    // 色の表は灰色のまま出てきたときだけ引く（ビューアと同じく、混合などで RGB になったらその色を出す）。
    let palette = recipe.palette.as_ref().filter(|_| float.channels == 1);
    let color = match palette {
        Some(_) => 3,
        None => float.channels,
    };
    let channels = color + usize::from(alpha);
    let mut data = Vec::with_capacity(float.width * float.height * channels);
    for (px, standard) in float
        .data
        .chunks_exact(float.channels)
        .zip(r.standard.as_chunks::<4>().0)
    {
        match palette {
            Some(p) => data.extend(p.colors[level(px[0])].map(|c| f32::from(c) / 255.0)),
            None => data.extend(px.iter().map(|v| v.clamp(0.0, 1.0))),
        }
        if alpha {
            data.push(f32::from(standard[3]) / 255.0);
        }
    }
    Ok(Picture {
        width: float.width,
        height: float.height,
        channels,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use vw_core::PixelData;
    use vw_pipeline::look::{BlackWhite, COLOR_MAP_OFF};
    use vw_pipeline::nodes::Node;

    fn recipe(chain: Vec<Node>, look: Look) -> Recipe {
        let palette = look.palette().unwrap();
        Recipe {
            values: Values {
                params: None,
                chain,
            },
            look,
            palette,
            develop: false,
            all_frames: false,
        }
    }

    fn gray16(values: Vec<u16>) -> VwImage {
        let width = values.len() as u32;
        VwImage::new(width, 1, ColorModel::Gray, PixelData::U16(values)).unwrap()
    }

    /// どのファイルも読もうとすると panic するデコーダ。
    struct Panics;

    impl vw_core::DecoderPlugin for Panics {
        fn name(&self) -> &str {
            "Panics"
        }
        fn extensions(&self) -> &[&str] {
            &["bad"]
        }
        fn probe(&self, _header: &[u8]) -> bool {
            false
        }
        fn decode(&self, _data: &[u8]) -> Result<VwImage, vw_core::DecodeError> {
            panic!("decoder bug")
        }
    }

    #[test]
    fn a_decoder_panic_is_an_error_for_that_file_only() {
        let dir = std::env::temp_dir().join(format!("vwbatch-panic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.bad");
        std::fs::write(&file, b"x").unwrap();
        let mut registry = Registry::new();
        registry.register(std::sync::Arc::new(Panics));
        let look = Look::default();
        let result = convert(&registry, &file, &recipe(Vec::new(), look));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(result.err().as_deref(), Some("internal error: decoder bug"));
    }

    #[test]
    fn the_saved_black_and_white_and_nodes_are_applied_in_the_viewer_order() {
        let look = Look {
            black_white: BlackWhite::Values {
                black: 1000.0,
                white: 3000.0,
            },
            color_map: Some(COLOR_MAP_OFF.into()),
            ..Look::default()
        };
        let mut image = gray16(vec![0, 1000, 2000, 3000, 9000]);
        let p = picture(&mut image, None, &recipe(vec![Node::Invert], look)).unwrap();
        assert_eq!(p.channels, 1);
        let rounded: Vec<f32> = p.data.iter().map(|v| (v * 100.0).round() / 100.0).collect();
        assert_eq!(rounded, [1.0, 1.0, 0.5, 0.0, 0.0]);
    }

    #[test]
    fn a_color_map_turns_gray_into_rgb_and_keeps_alpha() {
        let look = Look {
            color_map: Some("inferno".into()),
            ..Look::default()
        };
        let mut image =
            VwImage::new(1, 1, ColorModel::GrayAlpha, PixelData::U8(vec![255, 128])).unwrap();
        let p = picture(&mut image, None, &recipe(vec![], look)).unwrap();
        assert_eq!(p.channels, 4);
        let top = Palette::builtin("inferno").unwrap().colors[255];
        assert_eq!(p.data[..3], top.map(|c| f32::from(c) / 255.0));
        assert!((p.data[3] - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(p.without_alpha().channels, 3);
    }

    #[test]
    fn a_node_that_fails_is_reported_with_its_number() {
        let bad = Node::Expression {
            text: "nonsense(".into(),
        };
        let mut image = gray16(vec![0, 1]);
        let e = picture(
            &mut image,
            None,
            &recipe(vec![Node::Invert, bad], Look::default()),
        )
        .unwrap_err();
        assert!(e.starts_with("node 2:"), "{e}");
    }
}
