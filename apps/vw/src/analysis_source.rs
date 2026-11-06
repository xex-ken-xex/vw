//! 解析の元画像。解析のウインドウ（解析・評価・画素の変換・ROI のグラフ・線のプロファイル）が数える画像。
//!
//! 解析のウインドウはフル解像度の画像から数える。スロットの画像が替わってからフル解像度が
//! 届くまで（再生中はずっと）は、最後にフル解像度が届いた画像を見せ続け、読み込み中の印だけ出す。
//! ウインドウの中身を読み込み中の印に替えると、再生のたびにグラフと印が入れ替わって点滅して見えるため。
//! メタデータ欄の `metadata_slot` と同じ考え方。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use vw_core::VwImage;

use crate::loader::ImageInfo;

/// アクティブなスロットに表示中の画像。
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    pub path: &'a Path,
    pub info: &'a ImageInfo,
    /// フル解像度の画像。まだ届いていなければ `None`。
    pub image: Option<&'a Arc<VwImage>>,
}

/// 解析のウインドウに渡す画像。
#[derive(Clone, Copy)]
pub struct AnalysisImage<'a> {
    /// グラフを作る画像のパス。`stale` の間は表示中の画像と違うことがある。
    pub path: &'a Path,
    pub info: &'a ImageInfo,
    /// フル解像度の画像。一度も届いていなければ `None`。
    pub image: Option<&'a Arc<VwImage>>,
    /// 表示中の画像のフル解像度をまだ持っていない（前の画像を見せているか、何も見せられない）。
    pub stale: bool,
}

/// 最後にフル解像度が届いた画像を覚えておく。
#[derive(Default)]
pub struct AnalysisSource {
    held: Option<Held>,
}

struct Held {
    slot: u64,
    path: PathBuf,
    info: ImageInfo,
    image: Arc<VwImage>,
}

impl Held {
    fn is(&self, slot: u64, path: &Path, image: &Arc<VwImage>) -> bool {
        self.slot == slot && self.path == path && Arc::ptr_eq(&self.image, image)
    }
}

impl AnalysisSource {
    /// 毎フレーム、解析のウインドウを描く前に呼ぶ。`slot` はアクティブなスロットの番号。
    pub fn follow(&mut self, slot: Option<u64>, now: Option<Frame<'_>>) {
        let (Some(slot), Some(now)) = (slot, now) else {
            self.held = None;
            return;
        };
        match now.image {
            Some(image) => {
                if !self
                    .held
                    .as_ref()
                    .is_some_and(|h| h.is(slot, now.path, image))
                {
                    self.held = Some(Held {
                        slot,
                        path: now.path.to_path_buf(),
                        info: now.info.clone(),
                        image: image.clone(),
                    });
                }
            }
            // ほかのスロットに移ったら前のスロットの画像は見せない。別の画像のグラフを
            // 選んだスロットのものと取り違えるため。
            None if self.held.as_ref().is_some_and(|h| h.slot != slot) => self.held = None,
            None => {}
        }
    }

    /// 解析の元画像。
    pub fn view<'a>(&'a self, slot: u64, now: Frame<'a>) -> AnalysisImage<'a> {
        if now.image.is_some() {
            return AnalysisImage {
                path: now.path,
                info: now.info,
                image: now.image,
                stale: false,
            };
        }
        match self.held.as_ref().filter(|h| h.slot == slot) {
            Some(h) => AnalysisImage {
                path: &h.path,
                info: &h.info,
                image: Some(&h.image),
                stale: true,
            },
            None => AnalysisImage {
                path: now.path,
                info: now.info,
                image: None,
                stale: true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vw_core::{ColorModel, PixelData};

    fn image() -> Arc<VwImage> {
        Arc::new(VwImage::new(1, 1, ColorModel::Gray, PixelData::U8(vec![0])).unwrap())
    }

    fn frame<'a>(path: &'a str, info: &'a ImageInfo, image: Option<&'a Arc<VwImage>>) -> Frame<'a> {
        Frame {
            path: Path::new(path),
            info,
            image,
        }
    }

    #[test]
    fn shows_the_frame_itself_once_its_full_image_arrived() {
        let info = crate::viewer::test_support::info(1, 1, ColorModel::Gray);
        let a = image();
        let mut source = AnalysisSource::default();
        source.follow(Some(1), Some(frame("a?frame=0", &info, Some(&a))));
        let v = source.view(1, frame("a?frame=0", &info, Some(&a)));
        assert_eq!(v.path, Path::new("a?frame=0"));
        assert!(v.image.is_some_and(|i| Arc::ptr_eq(i, &a)));
        assert!(!v.stale);
    }

    #[test]
    fn keeps_the_last_full_image_while_the_next_frame_has_none() {
        let info = crate::viewer::test_support::info(1, 1, ColorModel::Gray);
        let a = image();
        let mut source = AnalysisSource::default();
        source.follow(Some(1), Some(frame("a?frame=0", &info, Some(&a))));
        // 再生で次のフレームに替わった。フル解像度はまだない。
        source.follow(Some(1), Some(frame("a?frame=1", &info, None)));
        let v = source.view(1, frame("a?frame=1", &info, None));
        assert_eq!(v.path, Path::new("a?frame=0"));
        assert!(v.image.is_some_and(|i| Arc::ptr_eq(i, &a)));
        assert!(v.stale);
    }

    #[test]
    fn switches_to_the_new_frame_when_its_full_image_arrives() {
        let info = crate::viewer::test_support::info(1, 1, ColorModel::Gray);
        let (a, b) = (image(), image());
        let mut source = AnalysisSource::default();
        source.follow(Some(1), Some(frame("a?frame=0", &info, Some(&a))));
        source.follow(Some(1), Some(frame("a?frame=1", &info, None)));
        source.follow(Some(1), Some(frame("a?frame=1", &info, Some(&b))));
        // 次に読み込み中になっても、見せるのは新しい方。
        source.follow(Some(1), Some(frame("a?frame=2", &info, None)));
        let v = source.view(1, frame("a?frame=2", &info, None));
        assert_eq!(v.path, Path::new("a?frame=1"));
        assert!(v.image.is_some_and(|i| Arc::ptr_eq(i, &b)));
    }

    #[test]
    fn forgets_the_image_of_another_slot() {
        let info = crate::viewer::test_support::info(1, 1, ColorModel::Gray);
        let a = image();
        let mut source = AnalysisSource::default();
        source.follow(Some(1), Some(frame("a", &info, Some(&a))));
        source.follow(Some(2), Some(frame("b", &info, None)));
        let v = source.view(2, frame("b", &info, None));
        assert_eq!(v.path, Path::new("b"));
        assert!(v.image.is_none());
        assert!(v.stale);
        // スロット 1 に戻っても、捨てた画像は出さない。
        assert!(source.view(1, frame("a", &info, None)).image.is_none());
    }

    #[test]
    fn forgets_the_image_when_nothing_is_shown() {
        let info = crate::viewer::test_support::info(1, 1, ColorModel::Gray);
        let a = image();
        let mut source = AnalysisSource::default();
        source.follow(Some(1), Some(frame("a", &info, Some(&a))));
        source.follow(Some(1), None);
        assert!(source.view(1, frame("a", &info, None)).image.is_none());
    }
}
