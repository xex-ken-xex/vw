//! HDF4 の科学データセット（SDS）を読み、NetCDF-3（CDF-5）のバイト列に書き直す。
//!
//! HDF4 の SDS は NetCDF-2 を元にした作り（名前の付いた次元・変数・属性）なので、NetCDF-3 の形に
//! そのまま移せる。書き直せば、変数の切り替え・値の戻し方・緯度・経度・メタデータを NetCDF と同じ
//! コードで扱える。HDF4 の読み方を NetCDF の部品の外にもう 1 つ作ると、同じ見せ方を 2 か所で
//! 保つことになるため、この形にした。
//!
//! 読むのは SD の API（MODIS などが使う）が書くファイル。古い画像（GR・RIS）と表（Vdata）は読まない。

mod cdf;
pub mod compress;

use std::collections::HashMap;

/// HDF4 のファイルの頭のバイト。
pub const MAGIC: [u8; 4] = [0x0e, 0x03, 0x13, 0x01];

pub fn is_hdf4(header: &[u8]) -> bool {
    header.starts_with(&MAGIC)
}

// 札（tag）の番号。
const DFTAG_LINKED: u16 = 20;
const DFTAG_COMPRESSED: u16 = 40;
const DFTAG_NT: u16 = 106;
const DFTAG_SDD: u16 = 701;
const DFTAG_SD: u16 = 702;
const DFTAG_NDG: u16 = 720;
const DFTAG_VH: u16 = 1962;
const DFTAG_VS: u16 = 1963;
const DFTAG_VG: u16 = 1965;
/// 特別な要素（圧縮・つないだブロック・チャンク）の札に立てるビット。
const SPECIAL: u16 = 0x4000;

const SPECIAL_LINKED: u16 = 1;
const SPECIAL_COMP: u16 = 3;
const SPECIAL_CHUNKED: u16 = 5;

/// 要素の場所。長さが `u32::MAX` の要素は、まだ書かれていない（空）。
#[derive(Debug, Clone, Copy)]
struct Place {
    offset: u32,
    length: u32,
}

/// 数の型（HDF4 の NT）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Char,
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    F32,
    F64,
}

impl Kind {
    /// HDF4 の型の番号から。リトルエンディアンの印（0x4000）と機械の並びの印（0x1000）は
    /// [`little_endian`] で見る。
    fn from_code(code: u32) -> Option<Self> {
        Some(match code & 0xff {
            3 | 4 => Self::Char,
            20 => Self::I8,
            21 => Self::U8,
            22 => Self::I16,
            23 => Self::U16,
            24 => Self::I32,
            25 => Self::U32,
            26 => Self::I64,
            27 => Self::U64,
            5 => Self::F32,
            6 => Self::F64,
            _ => return None,
        })
    }

    pub fn size(self) -> usize {
        match self {
            Self::Char | Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::I64 | Self::U64 | Self::F64 => 8,
        }
    }
}

/// 型の番号の、リトルエンディアン（0x4000）か機械の並び（0x1000）の印。機械の並びで書く
/// ファイルはほぼ x86 で作られるので、リトルエンディアンと見る。
fn little_endian(code: u32) -> bool {
    code & 0x5000 != 0
}

/// 型の付いた値の並び（ビッグエンディアン）。
#[derive(Debug, Clone, PartialEq)]
pub struct Values {
    pub kind: Kind,
    pub bytes: Vec<u8>,
}

impl Values {
    fn new(kind: Kind, mut bytes: Vec<u8>, little: bool) -> Self {
        if little && kind.size() > 1 {
            for v in bytes.chunks_exact_mut(kind.size()) {
                v.reverse();
            }
        }
        Self { kind, bytes }
    }

    fn from_f64(v: f64) -> Self {
        Self {
            kind: Kind::F64,
            bytes: v.to_be_bytes().to_vec(),
        }
    }

    /// 1 つ目の値を f64 で。
    fn first(&self) -> Option<f64> {
        let b = self.bytes.get(..self.kind.size())?;
        Some(match self.kind {
            Kind::Char => return None,
            Kind::I8 => f64::from(b[0] as i8),
            Kind::U8 => f64::from(b[0]),
            Kind::I16 => f64::from(i16::from_be_bytes([b[0], b[1]])),
            Kind::U16 => f64::from(u16::from_be_bytes([b[0], b[1]])),
            Kind::I32 => f64::from(i32::from_be_bytes([b[0], b[1], b[2], b[3]])),
            Kind::U32 => f64::from(u32::from_be_bytes([b[0], b[1], b[2], b[3]])),
            Kind::I64 => i64::from_be_bytes(b.try_into().ok()?) as f64,
            Kind::U64 => u64::from_be_bytes(b.try_into().ok()?) as f64,
            Kind::F32 => f64::from(f32::from_be_bytes([b[0], b[1], b[2], b[3]])),
            Kind::F64 => f64::from_be_bytes(b.try_into().ok()?),
        })
    }
}

/// 1 つの科学データセット。
#[derive(Debug, Clone)]
pub struct Sds {
    pub name: String,
    pub dims: Vec<(String, u64)>,
    pub attrs: Vec<(String, Values)>,
    pub data: Values,
}

/// ファイルの中身。
#[derive(Debug, Clone, Default)]
pub struct Contents {
    pub attrs: Vec<(String, Values)>,
    pub datasets: Vec<Sds>,
}

/// HDF4 を NetCDF-3（CDF-5）のバイト列にする。`limit` はデータの合計の上限（バイト）。
pub fn to_netcdf(data: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    let contents = read(data, limit)?;
    Ok(cdf::write(&contents))
}

/// 科学データセットと全体の属性を読む。
pub fn read(data: &[u8], limit: usize) -> Result<Contents, String> {
    let file = File::open(data)?;
    file.contents(limit)
}

struct File<'a> {
    data: &'a [u8],
    places: HashMap<(u16, u16), Place>,
    /// 目録の並びの (札, 番号)。Vgroup をファイルの順に見るため。
    order: Vec<(u16, u16)>,
}

fn be16(b: &[u8], at: usize) -> Result<u16, String> {
    b.get(at..at + 2)
        .map(|v| u16::from_be_bytes([v[0], v[1]]))
        .ok_or_else(|| "HDF4: data ends early".to_owned())
}

fn be32(b: &[u8], at: usize) -> Result<u32, String> {
    b.get(at..at + 4)
        .map(|v| u32::from_be_bytes([v[0], v[1], v[2], v[3]]))
        .ok_or_else(|| "HDF4: data ends early".to_owned())
}

/// 長さ（u16）の付いた文字列。読んだ後の位置も返す。
fn name_at(b: &[u8], at: usize) -> Result<(String, usize), String> {
    let n = usize::from(be16(b, at)?);
    let s = b
        .get(at + 2..at + 2 + n)
        .ok_or_else(|| "HDF4: name ends early".to_owned())?;
    Ok((String::from_utf8_lossy(s).into_owned(), at + 2 + n))
}

/// Vgroup（ほかの要素をまとめる入れ物）。
struct Vgroup {
    members: Vec<(u16, u16)>,
    name: String,
    class: String,
}

/// Vdata（表）の見出し。
struct VdataHeader {
    records: usize,
    record_size: usize,
    /// 欄ごとの (型の番号, 1 つの値の数, 名前)。
    fields: Vec<(u32, usize, String)>,
    name: String,
    class: String,
}

impl<'a> File<'a> {
    fn open(data: &'a [u8]) -> Result<Self, String> {
        if !is_hdf4(data) {
            return Err("HDF4: not an HDF4 file".into());
        }
        let mut places = HashMap::new();
        let mut order = Vec::new();
        let mut at = 4usize;
        let mut seen = std::collections::HashSet::new();
        while at != 0 {
            if !seen.insert(at) {
                return Err("HDF4: the directory loops".into());
            }
            let n = usize::from(be16(data, at)?);
            let next = be32(data, at + 2)? as usize;
            for i in 0..n {
                let p = at + 6 + 12 * i;
                let tag = be16(data, p)?;
                let r = be16(data, p + 2)?;
                if tag == 1 || tag == 0 {
                    continue;
                }
                places.insert(
                    (tag, r),
                    Place {
                        offset: be32(data, p + 4)?,
                        length: be32(data, p + 8)?,
                    },
                );
                order.push((tag, r));
            }
            at = next;
        }
        Ok(Self {
            data,
            places,
            order,
        })
    }

    /// 札と番号の要素の、そのままのバイト列と、特別な要素か。
    fn raw(&self, tag: u16, r: u16) -> Option<(&'a [u8], bool)> {
        let (place, special) = match self.places.get(&(tag, r)) {
            Some(p) => (p, false),
            None => (self.places.get(&(tag | SPECIAL, r))?, true),
        };
        if place.offset == u32::MAX || place.length == u32::MAX {
            return Some((&[], special));
        }
        let start = place.offset as usize;
        let bytes = self.data.get(start..start + place.length as usize)?;
        Some((bytes, special))
    }

    /// 要素の中身。圧縮とつないだブロックはほどく。チャンクは [`Self::chunked`] で読む。
    fn element(&self, tag: u16, r: u16, limit: usize) -> Result<Vec<u8>, String> {
        let (bytes, special) = self
            .raw(tag, r)
            .ok_or_else(|| format!("HDF4: element {tag}/{r} is missing"))?;
        if !special {
            return Ok(bytes.to_vec());
        }
        match be16(bytes, 0)? {
            SPECIAL_LINKED => self.linked(bytes, limit),
            SPECIAL_COMP => {
                let length = be32(bytes, 4)? as usize;
                if length > limit {
                    return Err("HDF4: data too large".into());
                }
                let comp_ref = be16(bytes, 8)?;
                let comp_type = be16(bytes, 12)?;
                let input = self.element(DFTAG_COMPRESSED, comp_ref, limit)?;
                compress::decompress(comp_type, &bytes[14..], &input, length)
            }
            SPECIAL_CHUNKED => Err("HDF4: chunked element read as a whole".into()),
            other => Err(format!("HDF4: special element {other} is not supported")),
        }
    }

    /// つないだブロック。見出しは全体の長さ・ブロックの長さ・表の 1 つのブロックの数・表の番号。
    fn linked(&self, header: &[u8], limit: usize) -> Result<Vec<u8>, String> {
        let length = be32(header, 2)? as usize;
        if length > limit {
            return Err("HDF4: data too large".into());
        }
        let per_table = be32(header, 10)? as usize;
        let mut table = be16(header, 14)?;
        let mut out = Vec::with_capacity(length);
        let mut tables = 0;
        while table != 0 && out.len() < length {
            tables += 1;
            if tables > 1 << 20 {
                return Err("HDF4: linked blocks loop".into());
            }
            let (t, _) = self
                .raw(DFTAG_LINKED, table)
                .ok_or("HDF4: linked block table is missing")?;
            let next = be16(t, 0)?;
            for i in 0..per_table {
                let r = be16(t, 2 + 2 * i)?;
                if r == 0 || out.len() >= length {
                    break;
                }
                let (block, _) = self
                    .raw(DFTAG_LINKED, r)
                    .ok_or("HDF4: linked block is missing")?;
                out.extend_from_slice(block);
            }
            table = next;
        }
        out.truncate(length);
        Ok(out)
    }

    fn vgroup(&self, r: u16) -> Result<Vgroup, String> {
        let (b, _) = self.raw(DFTAG_VG, r).ok_or("HDF4: Vgroup is missing")?;
        let n = usize::from(be16(b, 0)?);
        let mut members = Vec::with_capacity(n);
        for i in 0..n {
            members.push((be16(b, 2 + 2 * i)?, be16(b, 2 + 2 * n + 2 * i)?));
        }
        let (name, at) = name_at(b, 2 + 4 * n)?;
        let (class, _) = name_at(b, at)?;
        Ok(Vgroup {
            members,
            name,
            class,
        })
    }

    fn vdata_header(&self, r: u16) -> Result<VdataHeader, String> {
        let (b, _) = self.raw(DFTAG_VH, r).ok_or("HDF4: Vdata is missing")?;
        let records = be32(b, 2)? as usize;
        let record_size = usize::from(be16(b, 6)?);
        let n = usize::from(be16(b, 8)?);
        let mut types = Vec::with_capacity(n);
        let mut orders = Vec::with_capacity(n);
        for i in 0..n {
            types.push(u32::from(be16(b, 10 + 2 * i)?));
            orders.push(usize::from(be16(b, 10 + 6 * n + 2 * i)?));
        }
        let mut at = 10 + 8 * n;
        let mut fields = Vec::with_capacity(n);
        for i in 0..n {
            let (name, next) = name_at(b, at)?;
            fields.push((types[i], orders[i], name));
            at = next;
        }
        let (name, at) = name_at(b, at)?;
        let (class, _) = name_at(b, at)?;
        Ok(VdataHeader {
            records,
            record_size,
            fields,
            name,
            class,
        })
    }

    /// 属性（1 つの欄の Vdata）の名前と値。
    fn attribute(&self, r: u16, limit: usize) -> Result<(String, Values), String> {
        let h = self.vdata_header(r)?;
        let (code, order, _) = h.fields.first().ok_or("HDF4: attribute without a field")?;
        let kind = Kind::from_code(*code).ok_or("HDF4: attribute of an unknown type")?;
        let bytes = self.element(DFTAG_VS, r, limit)?;
        let n = (kind.size() * order * h.records).min(bytes.len());
        let mut values = Values::new(kind, bytes[..n].to_vec(), little_endian(*code));
        if kind == Kind::Char {
            while values.bytes.last() == Some(&0) {
                values.bytes.pop();
            }
        }
        Ok((h.name, values))
    }

    /// Vgroup の中の属性（クラス Attr0.0 の Vdata）。
    fn attributes(&self, members: &[(u16, u16)], limit: usize) -> Vec<(String, Values)> {
        members
            .iter()
            .filter(|(t, _)| *t == DFTAG_VH)
            .filter(|(_, r)| {
                self.vdata_header(*r)
                    .is_ok_and(|h| h.class.starts_with("Attr0."))
            })
            .filter_map(|(_, r)| self.attribute(*r, limit).ok())
            .collect()
    }

    fn contents(&self, limit: usize) -> Result<Contents, String> {
        let mut out = Contents::default();
        let mut used_ndgs = std::collections::HashSet::new();
        let mut budget = limit;
        for &(tag, r) in &self.order {
            if tag != DFTAG_VG {
                continue;
            }
            let Ok(vg) = self.vgroup(r) else { continue };
            match vg.class.as_str() {
                "CDF0.0" => out.attrs = self.attributes(&vg.members, limit),
                c if c.starts_with("Var0.") => {
                    let Some(sdd) = self.var_sdd(&vg, &mut used_ndgs) else {
                        continue;
                    };
                    let names: Vec<String> = vg
                        .members
                        .iter()
                        .filter(|(t, _)| *t == DFTAG_VG)
                        .filter_map(|(_, r)| self.vgroup(*r).ok())
                        .filter(|g| g.class.contains("Dim0."))
                        .map(|g| g.name)
                        .collect();
                    let attrs = self.attributes(&vg.members, limit);
                    if let Ok(sds) = self.sds(vg.name, sdd, names, attrs, &mut budget) {
                        out.datasets.push(sds);
                    }
                }
                _ => {}
            }
        }
        // Vgroup のない古い書き方（DFSD の API）。番号を名前にする。
        for &(tag, r) in &self.order {
            if tag != DFTAG_NDG || used_ndgs.contains(&r) {
                continue;
            }
            if let Some(sdd) = self.ndg_sdd(r)
                && let Ok(sds) =
                    self.sds(format!("SDS_{r}"), sdd, Vec::new(), Vec::new(), &mut budget)
            {
                out.datasets.push(sds);
            }
        }
        Ok(out)
    }

    /// Var の Vgroup の、次元の見出し（SDD）と中身（SD）の番号。
    fn var_sdd(
        &self,
        vg: &Vgroup,
        used: &mut std::collections::HashSet<u16>,
    ) -> Option<(u16, Option<u16>)> {
        if let Some(&(_, ndg)) = vg.members.iter().find(|(t, _)| *t == DFTAG_NDG) {
            used.insert(ndg);
            return self.ndg_sdd(ndg);
        }
        let sdd = vg.members.iter().find(|(t, _)| *t == DFTAG_SDD)?.1;
        let sd = vg.members.iter().find(|(t, _)| *t == DFTAG_SD).map(|m| m.1);
        Some((sdd, sd))
    }

    fn ndg_sdd(&self, ndg: u16) -> Option<(u16, Option<u16>)> {
        let (b, _) = self.raw(DFTAG_NDG, ndg)?;
        let pairs: Vec<(u16, u16)> = b
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                (
                    u16::from_be_bytes([p[0], p[1]]),
                    u16::from_be_bytes([p[2], p[3]]),
                )
            })
            .collect();
        let sdd = pairs.iter().find(|(t, _)| *t == DFTAG_SDD)?.1;
        let sd = pairs.iter().find(|(t, _)| *t == DFTAG_SD).map(|p| p.1);
        Some((sdd, sd))
    }

    fn sds(
        &self,
        name: String,
        (sdd, sd): (u16, Option<u16>),
        dim_names: Vec<String>,
        mut attrs: Vec<(String, Values)>,
        budget: &mut usize,
    ) -> Result<Sds, String> {
        let (b, _) = self.raw(DFTAG_SDD, sdd).ok_or("HDF4: SDD is missing")?;
        let rank = usize::from(be16(b, 0)?);
        let mut sizes = Vec::with_capacity(rank);
        for i in 0..rank {
            sizes.push(u64::from(be32(b, 2 + 4 * i)?));
        }
        let nt_ref = be16(b, 2 + 4 * rank + 2)?;
        let (nt, _) = self
            .raw(DFTAG_NT, nt_ref)
            .ok_or("HDF4: number type is missing")?;
        let code = u32::from(*nt.get(1).ok_or("HDF4: short number type")?)
            | if nt.get(3).is_some_and(|c| *c == 4) {
                0x4000
            } else {
                0
            };
        let kind = Kind::from_code(code).ok_or("HDF4: dataset of an unknown type")?;
        let count: u64 = sizes.iter().product();
        let total = usize::try_from(count * kind.size() as u64).map_err(|e| e.to_string())?;
        if total > *budget {
            return Err("HDF4: data too large".into());
        }
        *budget -= total;
        let bytes = match sd {
            Some(sd) => self.sds_bytes(sd, &sizes, kind, total)?,
            None => Vec::new(),
        };
        let mut data = Values::new(kind, bytes, little_endian(code));
        if data.bytes.len() < total {
            // 書かれていないデータは _FillValue（なければ 0）。
            let fill = attrs
                .iter()
                .find(|(n, v)| n == "_FillValue" && v.kind == kind)
                .map_or_else(|| vec![0; kind.size()], |(_, v)| v.bytes.clone());
            while data.bytes.len() < total {
                data.bytes.extend_from_slice(&fill);
            }
        }
        data.bytes.truncate(total);
        calibration(&mut attrs);
        let dims = sizes
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                let n = dim_names
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("fakeDim{i}"));
                (n, s)
            })
            .collect();
        Ok(Sds {
            name,
            dims,
            attrs,
            data,
        })
    }

    /// SD の中身（ファイルの並びのまま）。チャンクに分けたものは組み立てる。
    fn sds_bytes(
        &self,
        sd: u16,
        sizes: &[u64],
        kind: Kind,
        total: usize,
    ) -> Result<Vec<u8>, String> {
        let Some((head, special)) = self.raw(DFTAG_SD, sd) else {
            return Ok(Vec::new());
        };
        if special && be16(head, 0)? == SPECIAL_CHUNKED {
            return self.chunked(head, sizes, kind, total);
        }
        self.element(DFTAG_SD, sd, total)
    }

    /// チャンクに分けた SD。見出しのチャンクの表（Vdata）に、チャンクごとの位置と要素の番号がある。
    fn chunked(
        &self,
        head: &[u8],
        sizes: &[u64],
        kind: Kind,
        total: usize,
    ) -> Result<Vec<u8>, String> {
        let table = be16(head, 25)?;
        let rank = be32(head, 31)? as usize;
        if rank != sizes.len() {
            return Err("HDF4: chunk rank differs from the dataset".into());
        }
        let mut chunk = Vec::with_capacity(rank);
        for i in 0..rank {
            chunk.push(u64::from(be32(head, 35 + 12 * i + 8)?).max(1));
        }
        let fill_at = 35 + 12 * rank;
        let fill_len = be32(head, fill_at)? as usize;
        let fill = head
            .get(fill_at + 4..fill_at + 4 + fill_len)
            .filter(|f| f.len() == kind.size())
            .map_or_else(|| vec![0; kind.size()], <[u8]>::to_vec);
        let mut out = Vec::with_capacity(total);
        for _ in 0..total / kind.size() {
            out.extend_from_slice(&fill);
        }
        let h = self.vdata_header(table)?;
        let records = self.element(DFTAG_VS, table, total.max(1 << 20))?;
        let chunk_elems: u64 = chunk.iter().product();
        let chunk_bytes = usize::try_from(chunk_elems).map_err(|e| e.to_string())? * kind.size();
        for rec in records.chunks_exact(h.record_size.max(1)).take(h.records) {
            let origin: Vec<u64> = (0..rank)
                .map(|i| be32(rec, 4 * i).map(u64::from))
                .collect::<Result<_, _>>()?;
            let tag = be16(rec, 4 * rank)?;
            let r = be16(rec, 4 * rank + 2)?;
            let data = self.element(tag & !SPECIAL, r, chunk_bytes)?;
            if data.len() < chunk_bytes {
                return Err("HDF4: chunk is too short".into());
            }
            place_chunk(&mut out, &data, sizes, &chunk, &origin, kind.size());
        }
        Ok(out)
    }
}

/// チャンク（`origin` はチャンクの番号）の値を、全体の配列の場所に写す。端のチャンクのはみ出しは捨てる。
fn place_chunk(
    out: &mut [u8],
    chunk_data: &[u8],
    sizes: &[u64],
    chunk: &[u64],
    origin: &[u64],
    size: usize,
) {
    let rank = sizes.len();
    let start: Vec<u64> = origin.iter().zip(chunk).map(|(o, c)| o * c).collect();
    let chunk_elems: u64 = chunk.iter().product();
    let mut index = vec![0u64; rank];
    for k in 0..chunk_elems {
        let mut rest = k;
        for d in (0..rank).rev() {
            index[d] = rest % chunk[d];
            rest /= chunk[d];
        }
        let mut flat = 0u64;
        let mut inside = true;
        for d in 0..rank {
            let at = start[d] + index[d];
            if at >= sizes[d] {
                inside = false;
                break;
            }
            flat = flat * sizes[d] + at;
        }
        if inside {
            let from = k as usize * size;
            let to = flat as usize * size;
            out[to..to + size].copy_from_slice(&chunk_data[from..from + size]);
        }
    }
}

/// HDF4 の校正（SDsetcal）は 値 = scale_factor × (格納値 − add_offset) で、CF の
/// 値 = 格納値 × scale_factor + add_offset と引く順が逆。`calibrated_nt` があれば SDsetcal が
/// 書いたものなので、add_offset を CF の意味に直す。元の値は [`HDF4_ADD_OFFSET`] に残す。
fn calibration(attrs: &mut Vec<(String, Values)>) {
    if !attrs.iter().any(|(n, _)| n == "calibrated_nt") {
        return;
    }
    let get = |name: &str| {
        attrs
            .iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, v)| v.first())
    };
    let (Some(scale), Some(offset)) = (get("scale_factor"), get("add_offset")) else {
        return;
    };
    for (n, v) in attrs.iter_mut() {
        if n == "add_offset" {
            *v = Values::from_f64(-scale * offset);
        }
    }
    attrs.push((HDF4_ADD_OFFSET.into(), Values::from_f64(offset)));
}

/// HDF4 の校正の、元の add_offset（格納値から引く値）を書く属性の名前。
pub const HDF4_ADD_OFFSET: &str = "hdf4_add_offset";

#[cfg(test)]
mod tests;
