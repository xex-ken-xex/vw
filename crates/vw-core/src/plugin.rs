//! デコーダプラグインのインターフェースと、プラグインを束ねる [`Registry`]。

use std::fmt;
use std::io::Read as _;
use std::path::Path;
use std::sync::{Arc, RwLock};

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

    /// [`Self::extensions`] のほかに、拡張子で受け持つファイルか。設定で拡張子を変えられる形式
    /// （HDF5）だけが上書きする。静的な一覧では、設定を変えたときに差し替えられないため。
    fn claims_path(&self, _path: &Path) -> bool {
        false
    }

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

    /// アニメーションの各フレームを出しておく時間（ミリ秒）。フレームごとに違う形式（GIF・WebP）だけが返す。
    /// 既定は `None` で、再生はメタデータの平均の速さか設定の速さで回す。
    fn frame_delays(&self, _data: &[u8]) -> Option<Vec<u32>> {
        None
    }

    /// `frame` 番目（0 始まり）のフレームをデコードする。既定は 1 枚だけの形式向け。
    fn decode_frame(&self, data: &[u8], frame: usize) -> Result<VwImage, DecodeError> {
        match frame {
            0 => self.decode(data),
            _ => Err(DecodeError::Invalid(format!("no frame {frame}"))),
        }
    }

    /// ファイルの場所が分かっているときの [`Self::decode_frame`]。同じフォルダや近くの
    /// ファイルを使う形式（天体の画像の校正など）だけが上書きする。既定は場所を使わない。
    fn decode_frame_at(
        &self,
        _path: &Path,
        data: &[u8],
        frame: usize,
    ) -> Result<VwImage, DecodeError> {
        self.decode_frame(data, frame)
    }

    /// [`Self::decode`] の画像の代わりに、時間をかけて精細な画像を作る（カメラ RAW の
    /// センサーのデータの現像）。作れるかどうかは [`VwImage::develop_size`] で分かる。
    fn develop(&self, _data: &[u8]) -> Result<VwImage, DecodeError> {
        Err(DecodeError::Unsupported(format!(
            "{} images cannot be developed",
            self.name()
        )))
    }

    /// 画素をデコードせずにメタデータだけ読む。
    /// 既定の実装は全体をデコードするので、速くできる形式は上書きする。
    fn read_metadata(&self, data: &[u8]) -> Result<Metadata, DecodeError> {
        self.decode(data).map(|img| img.metadata)
    }

    /// 検索カタログに入れるメタデータ。既定は [`Self::read_metadata`] と同じ。
    /// メタデータ欄には見やすいよう選んだ項目だけを出し、検索にはすべてを使いたい形式
    /// （DICOM のタグ）が上書きする。
    fn read_all_metadata(&self, data: &[u8]) -> Result<Metadata, DecodeError> {
        self.read_metadata(data)
    }
}

/// 中身で形式を判定するときに読む先頭のバイト数。DICOM の印（128 バイト目から）が入る長さ。
const PROBE_BYTES: usize = 132;

/// どのプラグインの拡張子でもないファイルのうち、先頭を読んで中身で判定するものを、拡張子
/// （`.` を除いた部分、拡張子がなければ空文字列）に当てる正規表現で決める。大文字小文字は区別しない。
///
/// すべてのファイルを読まないのは、画像以外のファイルが多いフォルダやネットワークドライブで、
/// フォルダを開くたびに全ファイルを開くと遅くなるため。
#[derive(Debug, Clone)]
pub struct ProbeExtensions(regex::Regex);

impl ProbeExtensions {
    /// 既定。拡張子なし、数字だけ（`IMG.0314` など）、dcm、dicom、ima。DICOM がよく使う名前。
    pub const DEFAULT: &'static str = "^(|[0-9]+|dcm|dicom|ima)$";

    pub fn new(pattern: &str) -> Result<Self, String> {
        regex::RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
            .map(Self)
            .map_err(|e| e.to_string())
    }

    pub fn matches(&self, path: &Path) -> bool {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy())
            .unwrap_or_default();
        self.0.is_match(&ext)
    }
}

impl Default for ProbeExtensions {
    fn default() -> Self {
        Self::new(Self::DEFAULT).expect("the default pattern is valid")
    }
}

/// 利用できるデコーダプラグインの集合。
///
/// `Arc` で持つのは、読み込みスレッドごとに `Registry` を複製して渡すため。
#[derive(Clone, Default)]
pub struct Registry {
    plugins: Vec<Arc<dyn DecoderPlugin>>,
    /// 複製した `Registry` でも同じ設定を使うよう共有する。設定を変えたらすぐ全体に効かせるため。
    probe: Arc<RwLock<ProbeExtensions>>,
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

    /// 中身で判定する拡張子を変える。
    pub fn set_probe_extensions(&self, probe: ProbeExtensions) {
        *self.probe.write().unwrap_or_else(|e| e.into_inner()) = probe;
    }

    /// [`Self::supports_path`] に加えて、[`ProbeExtensions`] に当てはまるファイルは先頭を読んで
    /// 中身で判定する。DICOM は `IM0001` や `*.IMA`、装置ごとの連番など、拡張子が当てに
    /// ならない名前で保存されるため。拡張子で分かるファイルは開かないので、画像の多い
    /// フォルダでは遅くならない。
    pub fn supports_file(&self, path: &Path) -> bool {
        if self.supports_path(path) {
            return true;
        }
        if !self.needs_probe(path) {
            return false;
        }
        let mut header = Vec::with_capacity(PROBE_BYTES);
        let read = std::fs::File::open(path)
            .and_then(|f| std::io::Read::take(f, PROBE_BYTES as u64).read_to_end(&mut header));
        read.is_ok() && self.plugins.iter().any(|p| p.probe(&header))
    }

    /// 拡張子ではどのプラグインか分からず、中身を読んで判定するファイルか。
    pub fn needs_probe(&self, path: &Path) -> bool {
        self.by_extension(path).is_none()
            && self
                .probe
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .matches(path)
    }

    /// 拡張子は `.fits.gz` のように 2 つ重なったものも当てる（プラグインの拡張子が
    /// `fits.gz` なら）。`.gz` だけで選ぶと、画像でない gzip のファイルまで一覧に並ぶため。
    fn by_extension(&self, path: &Path) -> Option<&Arc<dyn DecoderPlugin>> {
        path.extension()?;
        let name = path.file_name()?.to_str()?.to_ascii_lowercase();
        self.plugins.iter().find(|p| {
            p.claims_path(path)
                || p.extensions().iter().any(|ext| {
                    name.len() > ext.len() + 1
                        && name.ends_with(ext)
                        && name.as_bytes()[name.len() - ext.len() - 1] == b'.'
                })
        })
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
        Ok((plugin.decode_frame_at(path, data, frame)?, plugin.name()))
    }

    /// プラグインを選んで [`DecoderPlugin::develop`] し、画像と使ったプラグイン名を返す。
    pub fn develop_bytes(&self, path: &Path, data: &[u8]) -> Result<(VwImage, &str), DecodeError> {
        let plugin = self
            .find(path, data)
            .ok_or_else(|| DecodeError::Unsupported(format!("no plugin for {}", path.display())))?;
        Ok((plugin.develop(data)?, plugin.name()))
    }

    /// プラグインを選んで [`DecoderPlugin::read_all_metadata`] し、メタデータと使ったプラグイン名を返す。
    pub fn read_all_metadata_bytes(
        &self,
        path: &Path,
        data: &[u8],
    ) -> Result<(Metadata, &str), DecodeError> {
        let plugin = self
            .find(path, data)
            .ok_or_else(|| DecodeError::Unsupported(format!("no plugin for {}", path.display())))?;
        Ok((plugin.read_all_metadata(data)?, plugin.name()))
    }

    /// ファイルの各フレームを出しておく時間（ミリ秒）。[`DecoderPlugin::frame_delays`]。
    pub fn frame_delays(&self, path: &Path, data: &[u8]) -> Option<Vec<u32>> {
        self.find(path, data)?.frame_delays(data)
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
    fn double_extensions_match_only_plugins_that_list_them() {
        let mut r = registry();
        r.register(Arc::new(Fake {
            name: "GZ",
            ext: &["ccc.gz"],
            magic: b"GZ",
            value: 3,
        }));
        assert!(r.supports_path(Path::new("x.CCC.gz")));
        assert!(!r.supports_path(Path::new("x.gz")));
        assert!(!r.supports_path(Path::new("x.ddd.gz")));
        assert!(!r.supports_path(Path::new(".aaa")));
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
    fn decoding_from_the_registry_tells_the_plugin_where_the_file_is() {
        struct Where;
        impl DecoderPlugin for Where {
            fn name(&self) -> &str {
                "WHERE"
            }
            fn extensions(&self) -> &[&str] {
                &["where"]
            }
            fn probe(&self, _header: &[u8]) -> bool {
                false
            }
            fn decode(&self, _data: &[u8]) -> Result<VwImage, DecodeError> {
                Err(DecodeError::Invalid("path not given".into()))
            }
            fn decode_frame_at(
                &self,
                path: &Path,
                _data: &[u8],
                _frame: usize,
            ) -> Result<VwImage, DecodeError> {
                let mut img = VwImage::new(1, 1, ColorModel::Gray, PixelData::U8(vec![0]))
                    .map_err(DecodeError::Invalid)?;
                img.metadata
                    .push("WHERE", "path", path.display().to_string());
                Ok(img)
            }
        }
        let mut r = Registry::new();
        r.register(Arc::new(Where));
        let (img, _) = r.decode_bytes(Path::new("/d/x.where"), b"").unwrap();
        assert_eq!(img.metadata.get("WHERE", "path"), Some("/d/x.where"));
    }

    #[test]
    fn files_with_probed_extensions_are_recognised_by_their_content() {
        let dir = std::env::temp_dir().join(format!("vw-core-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, data: &[u8]| {
            let p = dir.join(name);
            std::fs::write(&p, data).unwrap();
            p
        };
        let no_extension = write("IM0001", b"BBB...");
        let digits = write("IMG.0314", b"BBB...");
        let ima = write("scan.0036.IMA", b"BBB...");
        let other = write("scan.xyz", b"BBB...");
        let text = write("README", b"hello");
        let r = registry();
        assert!(r.supports_file(&no_extension));
        assert!(r.supports_file(&digits));
        assert!(r.supports_file(&ima));
        // 一覧にない拡張子は、中身が対応形式でも読まない。
        assert!(!r.supports_file(&other));
        assert!(!r.supports_file(&text));
        // プラグインの拡張子なら中身は見ない。
        assert!(r.supports_file(&dir.join("missing.aaa")));

        r.set_probe_extensions(ProbeExtensions::new("^xyz$").unwrap());
        assert!(r.supports_file(&other));
        assert!(!r.supports_file(&no_extension));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn probe_pattern_is_matched_against_the_extension_ignoring_case() {
        let p = ProbeExtensions::default();
        for yes in ["IM0001", "IMG.0314", "a.dcm", "a.DICOM", "PT.0036.IMA"] {
            assert!(p.matches(Path::new(yes)), "{yes}");
        }
        for no in ["a.txt", "a.0314a", "a.dcm2"] {
            assert!(!p.matches(Path::new(no)), "{no}");
        }
        assert!(ProbeExtensions::new("(").is_err());
    }

    /// 設定の正規表現のように、静的な一覧にない拡張子を受け持つテスト用プラグイン。
    struct Claims;

    impl DecoderPlugin for Claims {
        fn name(&self) -> &str {
            "CLAIMS"
        }
        fn extensions(&self) -> &[&str] {
            &[]
        }
        fn claims_path(&self, path: &Path) -> bool {
            path.extension().is_some_and(|e| e == "h5")
        }
        fn probe(&self, _header: &[u8]) -> bool {
            false
        }
        fn decode(&self, _data: &[u8]) -> Result<VwImage, DecodeError> {
            Err(DecodeError::Unsupported("test".into()))
        }
    }

    #[test]
    fn plugins_can_claim_extensions_outside_their_static_list() {
        let mut r = Registry::new();
        r.register(Arc::new(Claims));
        assert!(r.supports_path(Path::new("swath.h5")));
        assert!(!r.supports_path(Path::new("swath.h6")));
        assert_eq!(r.find(Path::new("a.h5"), b"").unwrap().name(), "CLAIMS");
    }

    #[test]
    fn empty_registry_supports_nothing() {
        let r = Registry::new();
        assert!(r.plugins().is_empty());
        assert!(!r.supports_path(Path::new("x.png")));
    }
}
