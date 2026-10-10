//! デコーダプラグインのインターフェースと、プラグインを束ねる [`Registry`]。

use std::fmt;
use std::io::Read as _;
use std::path::Path;
use std::sync::Arc;

use crate::image::{Metadata, VwImage};

#[derive(Debug)]
pub enum DecodeError {
    Io(std::io::Error),
    /// 対応するプラグインがない、またはプラグインが扱えない種類のファイル。
    Unsupported(String),
    /// ファイルが壊れている、または形式に反している。
    Invalid(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Io(e) => write!(f, "I/O error: {e}"),
            DecodeError::Unsupported(s) => write!(f, "unsupported: {s}"),
            DecodeError::Invalid(s) => write!(f, "invalid data: {s}"),
        }
    }
}

impl std::error::Error for DecodeError {}

impl From<std::io::Error> for DecodeError {
    fn from(e: std::io::Error) -> Self {
        DecodeError::Io(e)
    }
}

/// ファイル形式ごとのデコーダ。PNG, BMP, JPEG, DICOM, NEF, ARW などを
/// それぞれ 1 つのプラグインとして実装し、バイト列を [`VwImage`] に変換する。
///
/// 今はプラグインを静的リンクして [`Registry`] に登録している。
/// トレイトをオブジェクト安全に保ち、ジェネリクスや関連型を持たせていないのは、
/// 後で C ABI の薄いラッパーを被せて DLL / .so の動的プラグインにするため。
/// Rust のトレイトをそのまま動的ライブラリ越しに渡す方式は、
/// コンパイラのバージョン間で ABI が保証されないので採らない。
pub trait DecoderPlugin: Send + Sync {
    /// 表示用の短い名前（"PNG" など）。
    fn name(&self) -> &str;

    /// 対応する拡張子。小文字、ドットなし（`["png"]` など）。
    fn extensions(&self) -> &[&str];

    /// ファイル先頭のバイト列がこの形式に見えるか。
    /// `header` はファイル全体のこともあるが、先頭数十バイトしか来ない前提で判定する。
    fn probe(&self, header: &[u8]) -> bool;

    /// ファイル全体のバイト列をデコードする。
    fn decode(&self, data: &[u8]) -> Result<VwImage, DecodeError>;

    /// ファイルに入っている画像（フレーム）の数。動画のような DICOM のシネは 1 ファイルに
    /// 何百枚も入っている。既定は 1 枚。
    fn frame_count(&self, _data: &[u8]) -> Result<usize, DecodeError> {
        Ok(1)
    }

    /// `frame` 番目（0 始まり）のフレームをデコードする。既定は 1 枚だけの形式向け。
    fn decode_frame(&self, data: &[u8], frame: usize) -> Result<VwImage, DecodeError> {
        match frame {
            0 => self.decode(data),
            _ => Err(DecodeError::Invalid(format!("no frame {frame}"))),
        }
    }

    /// 画素をデコードせずにメタデータだけ読む。
    /// 既定の実装は全体をデコードするので、速くできる形式は上書きする。
    fn read_metadata(&self, data: &[u8]) -> Result<Metadata, DecodeError> {
        self.decode(data).map(|img| img.metadata)
    }
}

/// 中身で形式を判定するときに読む先頭のバイト数。DICOM の印（128 バイト目から）が入る長さ。
const PROBE_BYTES: usize = 132;

/// 利用できるデコーダプラグインの集合。
///
/// `Arc` で持つのは、読み込みスレッドごとに `Registry` を複製して渡すため。
#[derive(Clone, Default)]
pub struct Registry {
    plugins: Vec<Arc<dyn DecoderPlugin>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 先に登録したプラグインほど優先される。
    pub fn register(&mut self, plugin: Arc<dyn DecoderPlugin>) {
        self.plugins.push(plugin);
    }

    pub fn plugins(&self) -> &[Arc<dyn DecoderPlugin>] {
        &self.plugins
    }

    /// 拡張子だけで対応可否を判定する。フォルダ一覧の絞り込み用。
    /// 一覧の全ファイルを開いて中身を確認すると、大きなフォルダで遅くなるので拡張子に限る。
    pub fn supports_path(&self, path: &Path) -> bool {
        self.by_extension(path).is_some()
    }

    /// [`Self::supports_path`] に加えて、どのプラグインの拡張子でもないファイルは先頭を読んで
    /// 中身で判定する。DICOM は `IM0001` や `*.IMA`、装置ごとの連番など、拡張子が当てに
    /// ならない名前で保存されるため。拡張子で分かるファイルは開かないので、画像の多い
    /// フォルダでは遅くならない。
    pub fn supports_file(&self, path: &Path) -> bool {
        if !self.needs_probe(path) {
            return true;
        }
        let mut header = Vec::with_capacity(PROBE_BYTES);
        let read = std::fs::File::open(path)
            .and_then(|f| std::io::Read::take(f, PROBE_BYTES as u64).read_to_end(&mut header));
        read.is_ok() && self.plugins.iter().any(|p| p.probe(&header))
    }

    /// 拡張子ではどのプラグインか分からず、中身を見ないと対応しているか決まらないか。
    pub fn needs_probe(&self, path: &Path) -> bool {
        self.by_extension(path).is_none()
    }

    fn by_extension(&self, path: &Path) -> Option<&Arc<dyn DecoderPlugin>> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        self.plugins
            .iter()
            .find(|p| p.extensions().contains(&ext.as_str()))
    }

    /// 中身（マジックナンバー）で選び、どれも当てはまらなければ拡張子で選ぶ。
    /// 拡張子を優先しないのは、拡張子と中身が食い違うファイル（.jpg という名前の PNG など）が
    /// 実際によくあるため。
    pub fn find(&self, path: &Path, header: &[u8]) -> Option<&Arc<dyn DecoderPlugin>> {
        self.plugins
            .iter()
            .find(|p| p.probe(header))
            .or_else(|| self.by_extension(path))
    }

    /// プラグインを選んでデコードし、画像と使ったプラグイン名を返す。
    pub fn decode_bytes(&self, path: &Path, data: &[u8]) -> Result<(VwImage, &str), DecodeError> {
        self.decode_frame_bytes(path, data, 0)
    }

    /// [`Self::decode_bytes`] の、`frame` 番目のフレームを読む版。
    pub fn decode_frame_bytes(
        &self,
        path: &Path,
        data: &[u8],
        frame: usize,
    ) -> Result<(VwImage, &str), DecodeError> {
        let plugin = self
            .find(path, data)
            .ok_or_else(|| DecodeError::Unsupported(format!("no plugin for {}", path.display())))?;
        Ok((plugin.decode_frame(data, frame)?, plugin.name()))
    }

    /// ファイルのフレーム数。対応するプラグインがなければエラー。
    pub fn frame_count(&self, path: &Path, data: &[u8]) -> Result<usize, DecodeError> {
        self.find(path, data)
            .ok_or_else(|| DecodeError::Unsupported(format!("no plugin for {}", path.display())))?
            .frame_count(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::{ColorModel, PixelData};

    /// 先頭が `magic` のファイルを、画素値 `value` の 1x1 グレー画像として返すテスト用プラグイン。
    struct Fake {
        name: &'static str,
        ext: &'static [&'static str],
        magic: &'static [u8],
        value: u8,
    }

    impl DecoderPlugin for Fake {
        fn name(&self) -> &str {
            self.name
        }
        fn extensions(&self) -> &[&str] {
            self.ext
        }
        fn probe(&self, header: &[u8]) -> bool {
            header.starts_with(self.magic)
        }
        fn decode(&self, _data: &[u8]) -> Result<VwImage, DecodeError> {
            let mut img = VwImage::new(1, 1, ColorModel::Gray, PixelData::U8(vec![self.value]))
                .map_err(DecodeError::Invalid)?;
            img.metadata.push(self.name, "decoded", true);
            Ok(img)
        }
    }

    fn registry() -> Registry {
        let mut r = Registry::new();
        r.register(Arc::new(Fake {
            name: "AAA",
            ext: &["aaa"],
            magic: b"AAA",
            value: 1,
        }));
        r.register(Arc::new(Fake {
            name: "BBB",
            ext: &["bbb", "bb"],
            magic: b"BBB",
            value: 2,
        }));
        r
    }

    #[test]
    fn supports_path_matches_extension_case_insensitively() {
        let r = registry();
        assert!(r.supports_path(Path::new("x.aaa")));
        assert!(r.supports_path(Path::new("x.BB")));
        assert!(!r.supports_path(Path::new("x.ccc")));
        assert!(!r.supports_path(Path::new("no_extension")));
    }

    #[test]
    fn content_wins_over_extension() {
        let r = registry();
        let (_, name) = r
            .decode_bytes(Path::new("mislabelled.aaa"), b"BBB...")
            .unwrap();
        assert_eq!(name, "BBB");
    }

    #[test]
    fn extension_is_used_when_no_plugin_recognises_content() {
        let r = registry();
        let (_, name) = r.decode_bytes(Path::new("x.bbb"), b"????").unwrap();
        assert_eq!(name, "BBB");
    }

    #[test]
    fn unknown_file_is_unsupported() {
        let r = registry();
        let err = r.decode_bytes(Path::new("x.ccc"), b"????").unwrap_err();
        assert!(matches!(err, DecodeError::Unsupported(_)));
    }

    #[test]
    fn default_read_metadata_comes_from_decode() {
        let plugin = Fake {
            name: "AAA",
            ext: &["aaa"],
            magic: b"AAA",
            value: 1,
        };
        let m = plugin.read_metadata(b"AAA").unwrap();
        assert_eq!(m.get("AAA", "decoded"), Some("true"));
    }

    #[test]
    fn single_image_formats_have_one_frame_and_reject_others() {
        let r = registry();
        let path = Path::new("x.aaa");
        assert_eq!(r.frame_count(path, b"AAA").unwrap(), 1);
        assert!(r.decode_frame_bytes(path, b"AAA", 0).is_ok());
        assert!(matches!(
            r.decode_frame_bytes(path, b"AAA", 1),
            Err(DecodeError::Invalid(_))
        ));
    }

    #[test]
    fn files_with_unknown_or_no_extension_are_recognised_by_their_content() {
        let dir = std::env::temp_dir().join(format!("vw-core-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let known = dir.join("IM0001");
        let odd_extension = dir.join("scan.0314.IMA");
        let unknown = dir.join("README");
        std::fs::write(&known, b"BBB...").unwrap();
        std::fs::write(&odd_extension, b"BBB...").unwrap();
        std::fs::write(&unknown, b"hello").unwrap();
        let r = registry();
        assert!(r.supports_file(&known));
        assert!(r.supports_file(&odd_extension));
        assert!(!r.supports_file(&unknown));
        // プラグインの拡張子なら中身は見ない。
        assert!(r.supports_file(&dir.join("missing.aaa")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn empty_registry_supports_nothing() {
        let r = Registry::new();
        assert!(r.plugins().is_empty());
        assert!(!r.supports_path(Path::new("x.png")));
    }
}
