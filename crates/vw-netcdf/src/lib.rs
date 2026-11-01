//! NetCDF（NetCDF-3 と NetCDF-4）のデコーダプラグイン。読むだけ。
//!
//! - 数値で 2 次元以上の変数を画像にする（座標の変数と `bounds` の変数は除く）。見せる変数は
//!   [`Choices`] で選び、選んでいなければ面のいちばん大きい変数
//! - 縦と横は座標の変数の CF の軸（`axis`・`standard_name`・`units`）で決め、他の次元
//!   （時刻・高さ）は 1 枚ずつのフレームにする。札は [`vw_fits::PLANE`] と同じ鍵
//! - 画像は計測の値（`scale_factor`・`add_offset` の後）の F32 で、欠損（`_FillValue`・
//!   `missing_value`・`valid_range` の外）は NaN
//! - 縦の座標が下から上へ増える（南から北の緯度）なら上下を返し、北を上にする
//! - 黒と白は天体の画像と同じ決め方（[`vw_fits::stretch`]）
//!
//! ファイルはバイト列から開く。読み込みのクレートにはファイルをメモリマップで開く入口もあるが、
//! Windows ではマップしたままのファイルを移動・ごみ箱へ送る・上書きできなくなるため使わない。

pub mod geo;
mod layout;
mod meta;
pub mod projection;
mod values;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

use netcdf_reader::{NcFile, NcOpenOptions};
use vw_core::{ColorModel, DecodeError, DecoderPlugin, Metadata, PixelData, VwImage};
use vw_fits::Stretch;

pub use layout::Variable;
pub use values::Packing;

/// プラグインの名前（[`DecoderPlugin::name`]）。
pub const NAME: &str = "NetCDF";
/// メタデータの見出し。表示中の変数のまとめ。
pub const GROUP: &str = "NetCDF";
/// 表示中の変数の属性。
pub const VARIABLE_GROUP: &str = "NetCDF variable";
/// 画像にできる変数の一覧。
pub const VARIABLES_GROUP: &str = "NetCDF variables";
/// 全体の属性。
pub const GLOBAL_GROUP: &str = "NetCDF global attributes";

pub const FORMAT: &str = "Format";
pub const VARIABLE: &str = "Variable";
pub const DIMENSIONS: &str = "Dimensions";
pub const TYPE: &str = "Type";
/// 計測の値の単位（変数の `units`）。
pub const UNITS: &str = "units";
pub const SCALE_FACTOR: &str = "scale_factor";
pub const ADD_OFFSET: &str = "add_offset";
/// 北を上にするため上下を返したとき、[`ROWS`] に書く値。
pub const ROWS: &str = vw_fits::ROWS;
pub const ROWS_FLIPPED: &str = vw_fits::ROWS_FLIPPED;
/// 経度を −180〜180° に回したとき、この鍵に [`LONGITUDES_ROTATED`] を書く。
pub const LONGITUDES: &str = "Longitudes";
pub const LONGITUDES_ROTATED: &str = "Rotated to -180..180";
/// 偏差の変数と見て、色の表を勧めるとき（[`SUGGESTED_PALETTE`] の値）。
pub const COLOR_MAP: &str = "Suggested color map";
pub const SUGGESTED_PALETTE: &str = "coolwarm";

/// 対応する拡張子。HDF5 の拡張子（`.h5`）は、NetCDF でない HDF5 を開かないよう入れない。
pub const EXTENSIONS: [&str; 4] = ["nc", "nc4", "cdf", "netcdf"];

/// 表示の決め方。設定画面で変える。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    pub stretch: Stretch,
    pub low_percent: f32,
    pub high_percent: f32,
    /// 変数の全部の面に同じ黒と白を使う。時刻で明るさが変わると、変化を比べられないため。
    pub same_window_for_planes: bool,
    /// 縦の座標が下から上へ増えるなら上下を返し、北を上にする。
    pub north_up: bool,
    /// 欠損を描く色（[`VwImage::missing`]）。`None` は黒。
    pub missing: Option<[u8; 3]>,
    /// 経度が 0〜360° の格子を、−180〜180° に回す（地図と同じ並びにする）。
    pub rotate_longitude: bool,
    /// 偏差の変数（[`geo::is_anomaly`]）は、0 から対称の黒と白と、0 を真ん中にした色の表にする。
    pub auto_anomaly: bool,
    /// 画素ごとの緯度・経度（[`VwImage::geo`]）を付ける。
    pub coordinates: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            stretch: Stretch::Percentile,
            low_percent: 0.5,
            high_percent: 99.5,
            same_window_for_planes: true,
            north_up: true,
            missing: Some(vw_fits::MISSING_COLOR),
            rotate_longitude: false,
            auto_anomaly: true,
            coordinates: true,
        }
    }
}

/// プラグインとアプリで共有する [`Options`]。読み込みスレッドごとに複製した `Registry` でも、
/// 設定を変えたらすぐ同じ値を使うため。
#[derive(Debug, Clone, Default)]
pub struct SharedOptions(Arc<RwLock<Options>>);

impl SharedOptions {
    pub fn get(&self) -> Options {
        *self.0.read().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set(&self, options: Options) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = options;
    }
}

/// ファイルごとに選んだ変数。起動している間だけ覚える。
///
/// 鍵はファイルの中身から作る（[`file_key`]）。デコーダにはファイルの場所が届かないことがあり、
/// フレームの数（[`DecoderPlugin::frame_count`]）も中身だけから答えるため。
#[derive(Debug, Clone, Default)]
pub struct Choices(Arc<Mutex<HashMap<u64, String>>>);

impl Choices {
    pub fn get(&self, data: &[u8]) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&file_key(data))
            .cloned()
    }

    pub fn set(&self, data: &[u8], variable: &str) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(file_key(data), variable.to_owned());
    }
}

/// ファイルの中身の鍵。先頭 64 KiB（見出しとヘッダ）と長さ。全体を読むと大きなファイルで遅いため。
pub fn file_key(data: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    data[..data.len().min(64 << 10)].hash(&mut h);
    data.len().hash(&mut h);
    h.finish()
}

/// 面を送るたびに作り直すと重いもの（全部の面の黒と白、画素ごとの緯度・経度）を覚える。
/// 作れなかったことも覚える。
#[derive(Debug, Clone)]
struct Memo<T>(Arc<Mutex<HashMap<u64, T>>>, usize);

impl<T: Clone> Memo<T> {
    /// `keep` 個まで覚え、超えたら全部忘れる。
    fn new(keep: usize) -> Self {
        Self(Arc::default(), keep)
    }

    fn get_or(&self, key: u64, make: impl FnOnce() -> T) -> T {
        if let Some(v) = self.0.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return v.clone();
        }
        let value = make();
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() >= self.1 {
            map.clear();
        }
        map.insert(key, value.clone());
        value
    }
}

/// 黒と白。
type Window = Option<(f32, f32)>;

pub struct NetcdfPlugin {
    options: SharedOptions,
    choices: Choices,
    windows: Memo<Window>,
    /// 画素ごとの緯度・経度は、図法から作ると大きな格子で数十 MB になるので、少しだけ覚える。
    grids: Memo<Option<Arc<vw_core::geo::GeoGrid>>>,
}

impl Default for NetcdfPlugin {
    fn default() -> Self {
        Self::new(SharedOptions::default(), Choices::default())
    }
}

impl NetcdfPlugin {
    pub fn new(options: SharedOptions, choices: Choices) -> Self {
        Self {
            options,
            choices,
            windows: Memo::new(64),
            grids: Memo::new(4),
        }
    }
}

/// 先頭のバイトが NetCDF-3（`CDF\x01`・`\x02`・`\x05`）か HDF5（NetCDF-4 の入れ物）か。
pub fn is_netcdf(header: &[u8]) -> bool {
    matches!(header.get(..4), Some(b"CDF\x01" | b"CDF\x02" | b"CDF\x05"))
        || header.starts_with(b"\x89HDF\r\n\x1a\n")
}

fn open(data: &[u8]) -> Result<NcFile, DecodeError> {
    let options = NcOpenOptions {
        max_read_bytes: vw_core::limits::max_image_bytes(),
        ..NcOpenOptions::default()
    };
    NcFile::from_bytes_with_options(data, options)
        .map_err(|e| DecodeError::Invalid(format!("NetCDF: {e}")))
}

/// 画像にできる変数（[`Variable::path`]）の一覧。変数を切り替えるメニューとキーに使う。
pub fn variables(data: &[u8]) -> Result<Vec<Variable>, DecodeError> {
    Ok(layout::variables(&open(data)?))
}

impl NetcdfPlugin {
    /// 見せる変数。選んだものがなければ（別のファイルの鍵とぶつかったときも）既定の変数。
    fn chosen<'a>(
        &self,
        data: &[u8],
        variables: &'a [Variable],
    ) -> Result<&'a Variable, DecodeError> {
        let picked = self
            .choices
            .get(data)
            .and_then(|p| variables.iter().find(|v| v.path == p));
        picked
            .or_else(|| layout::default_variable(variables))
            .ok_or_else(|| {
                DecodeError::Unsupported("NetCDF file has no 2-D numeric variable".into())
            })
    }

    fn decode_plane(&self, data: &[u8], frame: usize) -> Result<VwImage, DecodeError> {
        let file = open(data)?;
        let variables = layout::variables(&file);
        let chosen = self.chosen(data, &variables)?;
        let planes = chosen.planes();
        if frame >= planes {
            return Err(DecodeError::Invalid(format!("no frame {frame}")));
        }
        let var = file
            .variable(&chosen.path)
            .map_err(|e| DecodeError::Invalid(format!("NetCDF: {e}")))?;
        let mut plane = values::read_plane(&file, chosen, var, frame)?;
        let mut options = self.options.get();
        let anomaly = options.auto_anomaly && geo::is_anomaly(var);
        if anomaly {
            options.stretch = Stretch::Symmetric;
        }
        let roll = options
            .rotate_longitude
            .then(|| geo::longitude_roll(&file, chosen))
            .flatten();
        if let Some(k) = roll {
            for row in plane.values.chunks_mut(plane.width) {
                row.rotate_left(k);
            }
        }
        let mut img = VwImage::new(
            plane.width as u32,
            plane.height as u32,
            ColorModel::Gray,
            PixelData::F32(plane.values),
        )
        .map_err(DecodeError::Invalid)?;
        let flipped = options.north_up && rows_go_up(&file, chosen);
        if flipped {
            img.apply_orientation(4);
        }
        let grid_key = window_key(data, &chosen.path, &(flipped, roll));
        if options.coordinates {
            img.geo = self.grids.get_or(grid_key, || {
                geo::grid(&file, chosen, flipped, roll).map(Arc::new)
            });
        }
        let same = planes > 1 && options.same_window_for_planes;
        img.window = if same {
            let key = window_key(data, &chosen.path, &format!("{options:?}"));
            self.windows
                .get_or(key, || planes_window(&file, chosen, var, &options))
        } else {
            None
        }
        .or_else(|| window(&img.data, &options));
        img.missing = options.missing;
        img.metadata = meta::metadata(&file, &variables, chosen);
        if let Some(label) = meta::plane_label(&file, chosen, frame) {
            img.metadata.push(GROUP, vw_fits::PLANE, label);
        }
        if flipped {
            img.metadata.push(GROUP, ROWS, ROWS_FLIPPED);
        }
        if roll.is_some() {
            img.metadata.push(GROUP, LONGITUDES, LONGITUDES_ROTATED);
        }
        if anomaly {
            img.metadata.push(GROUP, COLOR_MAP, SUGGESTED_PALETTE);
        }
        if same {
            img.metadata
                .push(GROUP, vw_fits::PLANES_WINDOW, vw_fits::PLANES_WINDOW_SAME);
        }
        img.metadata
            .push(GROUP, vw_fits::STRETCH, options.stretch.title());
        if options.stretch == Stretch::Percentile {
            for (key, percent) in [
                (vw_fits::BLACK_PERCENTILE, options.low_percent),
                (vw_fits::WHITE_PERCENTILE, options.high_percent),
            ] {
                img.metadata.push(GROUP, key, format!("{percent} %"));
            }
        }
        Ok(img)
    }
}

fn window(data: &PixelData, options: &Options) -> Option<(f32, f32)> {
    vw_fits::stretch::window(
        data,
        options.stretch,
        options.low_percent,
        options.high_percent,
    )
}

/// ファイル・変数・`extra`（決め方）ごとの、覚えておく値の鍵。
fn window_key(data: &[u8], path: &str, extra: &impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    file_key(data).hash(&mut h);
    path.hash(&mut h);
    extra.hash(&mut h);
    h.finish()
}

/// 黒と白を決めるのに見る面の数の上限。全部の面を読むと時刻の多いファイルで重いので、等間隔に選ぶ。
const WINDOW_PLANES: usize = 16;

/// 変数の面を [`WINDOW_PLANES`] 枚まで等間隔に読み、まとめた値から黒と白を決める。
fn planes_window(
    file: &NcFile,
    layout: &Variable,
    var: &netcdf_reader::NcVariable,
    options: &Options,
) -> Option<(f32, f32)> {
    let planes = layout.planes();
    let count = planes.min(WINDOW_PLANES);
    // 1 面あたりの値の数。合わせて 100 万程度にする（stretch が間引く数と同じくらい）。
    let per_plane = (1_000_000 / count).max(1);
    let mut all = Vec::with_capacity(per_plane * count);
    for k in 0..count {
        let plane = k * planes / count;
        let Ok(p) = values::read_plane(file, layout, var, plane) else {
            continue;
        };
        let step = (p.values.len() / per_plane).max(1);
        all.extend(p.values.iter().step_by(step));
    }
    window(&PixelData::F32(all), options)
}

/// 縦の次元の座標が、ファイルの行の順に増えるか（最初の行が南）。座標がなければ `false`。
fn rows_go_up(file: &NcFile, layout: &Variable) -> bool {
    let scope = layout::scope(file, &layout.path);
    let name = &layout.dims[layout.rows].0;
    let Some(depth) = scope.iter().rposition(|g| {
        g.variables
            .iter()
            .any(|v| v.is_coordinate_variable_for(name))
    }) else {
        return false;
    };
    let groups: Vec<&str> = scope[1..=depth].iter().map(|g| g.name.as_str()).collect();
    let path = if groups.is_empty() {
        name.clone()
    } else {
        format!("{}/{name}", groups.join("/"))
    };
    let Ok(v) = file.read_variable_as_f64(&path) else {
        return false;
    };
    let v: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    matches!((v.first(), v.last()), (Some(a), Some(b)) if a < b)
}

impl DecoderPlugin for NetcdfPlugin {
    fn name(&self) -> &str {
        NAME
    }

    fn extensions(&self) -> &[&str] {
        &EXTENSIONS
    }

    fn probe(&self, header: &[u8]) -> bool {
        is_netcdf(header)
    }

    fn decode(&self, data: &[u8]) -> Result<VwImage, DecodeError> {
        self.decode_plane(data, 0)
    }

    fn frame_count(&self, data: &[u8]) -> Result<usize, DecodeError> {
        let variables = variables(data)?;
        Ok(self.chosen(data, &variables)?.planes())
    }

    fn decode_frame(&self, data: &[u8], frame: usize) -> Result<VwImage, DecodeError> {
        self.decode_plane(data, frame)
    }

    fn decode_frame_at(
        &self,
        path: &Path,
        data: &[u8],
        frame: usize,
    ) -> Result<VwImage, DecodeError> {
        // HDF5 の頭は NetCDF でない HDF5（`.h5`）にもある。拡張子が NetCDF でないものは開かない。
        if data.starts_with(b"\x89HDF") && !has_netcdf_extension(path) {
            return Err(DecodeError::Unsupported(format!(
                "HDF5 file {} is not opened as NetCDF",
                path.display()
            )));
        }
        self.decode_plane(data, frame)
    }

    fn read_metadata(&self, data: &[u8]) -> Result<Metadata, DecodeError> {
        let file = open(data)?;
        let variables = layout::variables(&file);
        let chosen = self.chosen(data, &variables)?;
        Ok(meta::metadata(&file, &variables, chosen))
    }

    fn read_all_metadata(&self, data: &[u8]) -> Result<Metadata, DecodeError> {
        let file = open(data)?;
        Ok(meta::catalog(&file, &layout::variables(&file)))
    }
}

/// 拡張子が [`EXTENSIONS`] のどれか。
pub fn has_netcdf_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

#[cfg(test)]
mod tests;
