//! tile 圧縮の画像（FITS の規約「Tiled Image Compression」、`.fz`）の展開。
//!
//! 画像を tile に分け、BINTABLE の 1 行に 1 tile を圧縮して入れたもの。RICE_1、GZIP_1、GZIP_2、
//! NOCOMPRESS を展開する。HCOMPRESS_1 と PLIO_1 は展開できない。
//!
//! fitsrs の展開を使わないのは、fitsrs の RICE が 1 画素 4 バイトの tile しか読めず、天体カメラの
//! `.fz`（16bit を 2 バイトで RICE 圧縮したもの）で、値が化けるか、桁あふれで落ちるため。
//! 展開の手順は CFITSIO（ricecomp.c、imcompress.c）と同じにしてある。
//!
//! 手順を移したので、CFITSIO の告知を残す（配布物では THIRD-PARTY-LICENSES.txt にも載せる）。
//!
//! Copyright (Unpublished-all rights reserved under the copyright laws of the United States), U.S. Government as represented by the Administrator of the National Aeronautics and Space Administration. No copyright is claimed in the United States under Title 17, U.S. Code.
//!
//! Permission to freely use, copy, modify, and distribute this software and its documentation without fee is hereby granted, provided that this copyright notice and disclaimer of warranty appears in all copies.
//!
//! DISCLAIMER:
//!
//! THE SOFTWARE IS PROVIDED 'AS IS' WITHOUT ANY WARRANTY OF ANY KIND, EITHER EXPRESSED, IMPLIED, OR STATUTORY, INCLUDING, BUT NOT LIMITED TO, ANY WARRANTY THAT THE SOFTWARE WILL CONFORM TO SPECIFICATIONS, ANY IMPLIED WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE, AND FREEDOM FROM INFRINGEMENT, AND ANY WARRANTY THAT THE DOCUMENTATION WILL CONFORM TO THE SOFTWARE, OR ANY WARRANTY THAT THE SOFTWARE WILL BE ERROR FREE. IN NO EVENT SHALL NASA BE LIABLE FOR ANY DAMAGES, INCLUDING, BUT NOT LIMITED TO, DIRECT, INDIRECT, SPECIAL OR CONSEQUENTIAL DAMAGES, ARISING OUT OF, RESULTING FROM, OR IN ANY WAY CONNECTED WITH THIS SOFTWARE, WHETHER OR NOT BASED UPON WARRANTY, CONTRACT, TORT , OR OTHERWISE, WHETHER OR NOT INJURY WAS SUSTAINED BY PERSONS OR PROPERTY OR OTHERWISE, AND WHETHER OR NOT LOSS WAS SUSTAINED FROM, OR AROSE OUT OF THE RESULTS OF, OR USE OF, THE SOFTWARE OR SERVICES PROVIDED HEREUNDER."

use std::io::Read;

use vw_core::DecodeError;

use crate::header::Hdu;

/// 展開できる圧縮の方式。
pub const METHODS: &[&str] = &["RICE_1", "RICE_ONE", "GZIP_1", "GZIP_2", "NOCOMPRESS"];

/// 1 面分の値。整数の画像は格納値（`BSCALE`・`BZERO` をかける前）、実数の画像は実際の値。
pub enum Plane {
    Integers(Vec<i64>),
    Floats(Vec<f32>),
}

fn invalid(what: &str) -> DecodeError {
    DecodeError::Invalid(format!("FITS compressed image: {what}"))
}

/// BINTABLE の列 1 つの、行の中での位置と形。
#[derive(Debug, Clone, Copy)]
struct Column {
    offset: usize,
    /// 型の文字（`P`・`Q` は可変長の配列で、その中身の型は `array`）。
    kind: u8,
    array: u8,
}

fn columns(hdu: &Hdu) -> Result<Vec<(String, Column)>, DecodeError> {
    let fields = hdu.int("TFIELDS").unwrap_or(0);
    if !(0..=999).contains(&fields) {
        return Err(invalid("broken TFIELDS"));
    }
    let mut out = Vec::new();
    let mut offset = 0usize;
    for i in 1..=fields {
        let form = hdu
            .get(&format!("TFORM{i}"))
            .ok_or_else(|| invalid("missing TFORM"))?
            .trim()
            .to_ascii_uppercase();
        let digits = form.bytes().take_while(u8::is_ascii_digit).count();
        let repeat: usize = if digits == 0 {
            1
        } else {
            form[..digits]
                .parse()
                .map_err(|_| invalid("broken TFORM"))?
        };
        let rest = &form.as_bytes()[digits..];
        let kind = *rest.first().ok_or_else(|| invalid("broken TFORM"))?;
        let size = match kind {
            b'L' | b'B' | b'A' => 1,
            b'X' => 0,
            b'I' => 2,
            b'J' | b'E' => 4,
            b'K' | b'D' | b'C' | b'P' => 8,
            b'M' | b'Q' => 16,
            _ => return Err(invalid("unknown TFORM")),
        };
        let width = if kind == b'X' {
            repeat.div_ceil(8)
        } else {
            repeat
                .checked_mul(size)
                .ok_or_else(|| invalid("broken TFORM"))?
        };
        let name = hdu
            .get(&format!("TTYPE{i}"))
            .unwrap_or_default()
            .trim()
            .to_ascii_uppercase();
        out.push((
            name,
            Column {
                offset,
                kind,
                array: rest.get(1).copied().unwrap_or(b'B'),
            },
        ));
        offset = offset
            .checked_add(width)
            .ok_or_else(|| invalid("broken TFORM"))?;
    }
    Ok(out)
}

/// 圧縮の決まりと、BINTABLE の置き場所。
struct Table<'a> {
    data: &'a [u8],
    row_len: usize,
    heap: usize,
    columns: Vec<(String, Column)>,
}

impl<'a> Table<'a> {
    fn column(&self, name: &str) -> Option<Column> {
        self.columns
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, c)| *c)
    }

    fn cell(&self, row: usize, column: Column, len: usize) -> Result<&'a [u8], DecodeError> {
        let at = row
            .checked_mul(self.row_len)
            .and_then(|r| r.checked_add(column.offset))
            .ok_or_else(|| invalid("broken table"))?;
        self.data
            .get(at..at + len)
            .ok_or_else(|| invalid("truncated table"))
    }

    /// 可変長の配列の中身（ヒープの上のバイト列）。
    fn array(&self, row: usize, column: Column) -> Result<&'a [u8], DecodeError> {
        let (count, offset) = match column.kind {
            b'P' => {
                let c = self.cell(row, column, 8)?;
                (
                    u32::from_be_bytes(c[..4].try_into().unwrap()) as usize,
                    u32::from_be_bytes(c[4..].try_into().unwrap()) as usize,
                )
            }
            b'Q' => {
                let c = self.cell(row, column, 16)?;
                (
                    u64::from_be_bytes(c[..8].try_into().unwrap()) as usize,
                    u64::from_be_bytes(c[8..].try_into().unwrap()) as usize,
                )
            }
            _ => return Err(invalid("compressed column is not an array")),
        };
        let size = match column.array {
            b'B' => 1,
            b'I' => 2,
            b'J' | b'E' => 4,
            b'K' | b'D' => 8,
            _ => return Err(invalid("unknown array type")),
        };
        let start = self
            .heap
            .checked_add(offset)
            .ok_or_else(|| invalid("broken heap"))?;
        let len = count
            .checked_mul(size)
            .ok_or_else(|| invalid("broken heap"))?;
        self.data
            .get(
                start
                    ..start
                        .checked_add(len)
                        .ok_or_else(|| invalid("broken heap"))?,
            )
            .ok_or_else(|| invalid("truncated heap"))
    }

    fn f64_cell(&self, row: usize, column: Option<Column>) -> Result<Option<f64>, DecodeError> {
        let Some(c) = column else { return Ok(None) };
        Ok(Some(match c.kind {
            b'D' => f64::from_be_bytes(self.cell(row, c, 8)?.try_into().unwrap()),
            b'E' => f32::from_be_bytes(self.cell(row, c, 4)?.try_into().unwrap()) as f64,
            _ => return Err(invalid("unknown ZSCALE / ZZERO type")),
        }))
    }

    fn i64_cell(&self, row: usize, column: Option<Column>) -> Result<Option<i64>, DecodeError> {
        let Some(c) = column else { return Ok(None) };
        Ok(Some(match c.kind {
            b'J' => i32::from_be_bytes(self.cell(row, c, 4)?.try_into().unwrap()) as i64,
            b'K' => i64::from_be_bytes(self.cell(row, c, 8)?.try_into().unwrap()),
            b'I' => i16::from_be_bytes(self.cell(row, c, 2)?.try_into().unwrap()) as i64,
            _ => return Err(invalid("unknown ZBLANK type")),
        }))
    }
}

/// `ZNAMEi = name` の `ZVALi`。
fn zval(hdu: &Hdu, name: &str) -> Option<i64> {
    (1..=999).find_map(|i| {
        let n = hdu.get(&format!("ZNAME{i}"))?;
        (n.trim().eq_ignore_ascii_case(name))
            .then(|| hdu.int(&format!("ZVAL{i}")))
            .flatten()
    })
}

/// 画像 `axes`（`ZNAXISn`）の `plane` 面目（3 次元目以降をまとめた番号）を展開する。
pub fn plane(
    data: &[u8],
    hdu: &Hdu,
    axes: &[usize],
    bitpix: i64,
    plane: usize,
) -> Result<Plane, DecodeError> {
    let method = hdu.get("ZCMPTYPE").unwrap_or_default().trim().to_owned();
    if !METHODS.contains(&method.as_str()) {
        return Err(DecodeError::Unsupported(format!(
            "FITS compression {method} is not supported"
        )));
    }
    let row_len = usize::try_from(hdu.int("NAXIS1").unwrap_or(0)).map_err(|_| invalid("NAXIS1"))?;
    let rows = usize::try_from(hdu.int("NAXIS2").unwrap_or(0)).map_err(|_| invalid("NAXIS2"))?;
    let main = row_len
        .checked_mul(rows)
        .ok_or_else(|| invalid("table too large"))?;
    let theap =
        usize::try_from(hdu.int("THEAP").unwrap_or(main as i64)).map_err(|_| invalid("THEAP"))?;
    let table = Table {
        data: data
            .get(hdu.data_start..hdu.data_start.saturating_add(hdu.data_len))
            .ok_or_else(|| DecodeError::Invalid("FITS file is truncated".into()))?,
        row_len,
        heap: theap,
        columns: columns(hdu)?,
    };
    let compressed = table
        .column("COMPRESSED_DATA")
        .ok_or_else(|| invalid("no COMPRESSED_DATA column"))?;
    let gzip_fallback = table.column("GZIP_COMPRESSED_DATA");
    let uncompressed = table.column("UNCOMPRESSED_DATA");
    let zscale = table.column("ZSCALE");
    let zzero = table.column("ZZERO");
    let zblank_column = table.column("ZBLANK");
    let zblank_key = hdu.int("ZBLANK");
    let quantized = bitpix < 0 && zscale.is_some() && zzero.is_some();
    let dither = Dither::of(hdu);

    // tile の大きさ（既定は 1 行ずつ）と、各次元の tile の数。
    let tile: Vec<usize> = (0..axes.len())
        .map(|i| {
            hdu.int(&format!("ZTILE{}", i + 1))
                .and_then(|v| usize::try_from(v).ok())
                .filter(|&v| v > 0)
                .unwrap_or(if i == 0 { axes[0] } else { 1 })
        })
        .collect();
    let counts: Vec<usize> = axes
        .iter()
        .zip(&tile)
        .map(|(a, t)| a.div_ceil(*t))
        .collect();
    let total_tiles = counts
        .iter()
        .try_fold(1usize, |n, &c| n.checked_mul(c))
        .ok_or_else(|| invalid("too many tiles"))?;
    if total_tiles > rows {
        return Err(DecodeError::Invalid("FITS file is truncated".into()));
    }
    let (w, h) = (axes[0], axes[1]);
    // 求める面の、3 次元目以降の位置。
    let mut want = Vec::with_capacity(axes.len().saturating_sub(2));
    let mut rest = plane;
    for &a in &axes[2..] {
        want.push(rest % a);
        rest /= a;
    }

    let mut ints = if bitpix > 0 {
        vec![0i64; w * h]
    } else {
        Vec::new()
    };
    let mut floats = if bitpix < 0 {
        vec![0f32; w * h]
    } else {
        Vec::new()
    };
    let mut index = vec![0usize; axes.len()];
    for row in 0..total_tiles {
        // tile の各次元の位置 index[k] から、始まりと大きさ。
        let origin: Vec<usize> = index.iter().zip(&tile).map(|(i, t)| i * t).collect();
        let size: Vec<usize> = origin
            .iter()
            .zip(axes)
            .zip(&tile)
            .map(|((o, a), t)| (*t).min(a - o))
            .collect();
        let covers = want
            .iter()
            .enumerate()
            .all(|(k, &c)| (origin[k + 2]..origin[k + 2] + size[k + 2]).contains(&c));
        if covers {
            let pixels: usize = size.iter().product();
            let values = tile_values(
                &table,
                hdu,
                &method,
                row,
                pixels,
                bitpix,
                quantized,
                Columns {
                    compressed,
                    gzip_fallback,
                    uncompressed,
                    zscale,
                    zzero,
                    zblank: zblank_column,
                },
                zblank_key,
                &dither,
            )?;
            // tile の中の画素を、面の中の位置へ置く。
            let mut at = vec![0usize; axes.len()];
            for i in 0..pixels {
                let mut r = i;
                for (k, s) in size.iter().enumerate() {
                    at[k] = origin[k] + r % s;
                    r /= s;
                }
                if at[2..].iter().zip(&want).all(|(a, w)| a == w) {
                    let dst = at[1] * w + at[0];
                    match &values {
                        Plane::Integers(v) => ints[dst] = v[i],
                        Plane::Floats(v) => floats[dst] = v[i],
                    }
                }
            }
        }
        // 次の tile（1 次元目が速く回る）。
        for (k, c) in counts.iter().enumerate() {
            index[k] += 1;
            if index[k] < *c {
                break;
            }
            index[k] = 0;
        }
    }
    Ok(if bitpix > 0 {
        Plane::Integers(ints)
    } else {
        Plane::Floats(floats)
    })
}

#[derive(Clone, Copy)]
struct Columns {
    compressed: Column,
    gzip_fallback: Option<Column>,
    uncompressed: Option<Column>,
    zscale: Option<Column>,
    zzero: Option<Column>,
    zblank: Option<Column>,
}

#[allow(clippy::too_many_arguments)]
fn tile_values(
    table: &Table,
    hdu: &Hdu,
    method: &str,
    row: usize,
    pixels: usize,
    bitpix: i64,
    quantized: bool,
    c: Columns,
    zblank_key: Option<i64>,
    dither: &Dither,
) -> Result<Plane, DecodeError> {
    let bytes = table.array(row, c.compressed)?;
    // 量子化できなかった実数の tile は、別の列に gzip か無圧縮で入っている。
    if bitpix < 0 && bytes.is_empty() {
        if let Some(g) = c.gzip_fallback {
            let raw = gunzip(
                table.array(row, g)?,
                pixels * (bitpix.unsigned_abs() as usize / 8),
            )?;
            return Ok(Plane::Floats(floats_be(&raw, bitpix, pixels)?));
        }
        if let Some(u) = c.uncompressed {
            let raw = table.array(row, u)?;
            let size = if u.array == b'D' { -64 } else { -32 };
            return Ok(Plane::Floats(floats_be(raw, size, pixels)?));
        }
        return Err(invalid("empty tile"));
    }
    if bitpix < 0 && !quantized {
        // 量子化しない実数（ZQUANTIZ = 'NONE'）は、実数のまま圧縮されている。
        let size = bitpix.unsigned_abs() as usize / 8;
        let raw = match method {
            "GZIP_1" => gunzip(bytes, pixels * size)?,
            "GZIP_2" => unshuffle(&gunzip(bytes, pixels * size)?, size),
            "NOCOMPRESS" => bytes.to_vec(),
            _ => return Err(invalid("RICE needs quantized floats")),
        };
        return Ok(Plane::Floats(floats_be(&raw, bitpix, pixels)?));
    }
    // ここから整数（量子化した実数も 4 バイトの整数として入っている）。
    let int_bytes = if bitpix < 0 { 4 } else { bitpix as usize / 8 };
    let ints: Vec<i64> = match method {
        "RICE_1" | "RICE_ONE" => {
            let bytepix = zval(hdu, "BYTEPIX").unwrap_or(int_bytes as i64);
            let block = zval(hdu, "BLOCKSIZE").unwrap_or(32);
            rice(bytes, pixels, bytepix as usize, block as usize)?
        }
        "GZIP_1" => ints_be(&gunzip(bytes, pixels * int_bytes)?, int_bytes, pixels)?,
        "GZIP_2" => ints_be(
            &unshuffle(&gunzip(bytes, pixels * int_bytes)?, int_bytes),
            int_bytes,
            pixels,
        )?,
        _ => ints_be(bytes, int_bytes, pixels)?,
    };
    if bitpix > 0 {
        return Ok(Plane::Integers(ints));
    }
    let scale = table.f64_cell(row, c.zscale)?.unwrap_or(1.0);
    let zero = table.f64_cell(row, c.zzero)?.unwrap_or(0.0);
    let blank = table.i64_cell(row, c.zblank)?.or(zblank_key);
    Ok(Plane::Floats(
        dither.unquantize(&ints, row, scale, zero, blank),
    ))
}

fn gunzip(bytes: &[u8], expected: usize) -> Result<Vec<u8>, DecodeError> {
    let mut out = Vec::with_capacity(expected);
    flate2::read::GzDecoder::new(bytes)
        .take(expected as u64)
        .read_to_end(&mut out)
        .map_err(|e| invalid(&format!("gzip: {e}")))?;
    Ok(out)
}

/// GZIP_2 はバイトを並べ替えてから圧縮してある（全画素の 1 バイト目、2 バイト目…の順）。
fn unshuffle(raw: &[u8], size: usize) -> Vec<u8> {
    let n = raw.len() / size.max(1);
    let mut out = vec![0; n * size];
    for (i, px) in out.chunks_exact_mut(size).enumerate() {
        for (j, b) in px.iter_mut().enumerate() {
            *b = raw[j * n + i];
        }
    }
    out
}

fn ints_be(raw: &[u8], size: usize, pixels: usize) -> Result<Vec<i64>, DecodeError> {
    if raw.len() < pixels * size {
        return Err(invalid("short tile"));
    }
    Ok(raw
        .chunks_exact(size)
        .take(pixels)
        .map(|c| match size {
            1 => c[0] as i64,
            2 => i16::from_be_bytes([c[0], c[1]]) as i64,
            4 => i32::from_be_bytes([c[0], c[1], c[2], c[3]]) as i64,
            _ => i64::from_be_bytes(c.try_into().unwrap_or([0; 8])),
        })
        .collect())
}

fn floats_be(raw: &[u8], bitpix: i64, pixels: usize) -> Result<Vec<f32>, DecodeError> {
    let size = bitpix.unsigned_abs() as usize / 8;
    if raw.len() < pixels * size {
        return Err(invalid("short tile"));
    }
    Ok(raw
        .chunks_exact(size)
        .take(pixels)
        .map(|c| match size {
            4 => f32::from_be_bytes([c[0], c[1], c[2], c[3]]),
            _ => f64::from_be_bytes(c.try_into().unwrap_or([0; 8])) as f32,
        })
        .collect())
}

/// RICE の展開（CFITSIO の fits_rdecomp / fits_rdecomp_short / fits_rdecomp_byte）。
/// `bytepix` は 1 画素のバイト数（1、2、4）。値は `bytepix` の幅で符号付きに戻す
/// （1 バイトは符号なし、FITS の 8bit 画像は符号なしのため）。
fn rice(
    bytes: &[u8],
    pixels: usize,
    bytepix: usize,
    block: usize,
) -> Result<Vec<i64>, DecodeError> {
    let (fsbits, fsmax): (i32, i64) = match bytepix {
        1 => (3, 6),
        2 => (4, 14),
        4 => (5, 25),
        _ => return Err(invalid("RICE BYTEPIX must be 1, 2 or 4")),
    };
    if block == 0 {
        return Err(invalid("RICE BLOCKSIZE is 0"));
    }
    let bbits = 1i32 << fsbits;
    let mask: u64 = if bytepix == 4 {
        u32::MAX as u64
    } else {
        (1 << (8 * bytepix)) - 1
    };
    let short = || invalid("RICE data is short");
    let mut c = 0usize;
    let mut next = || -> Result<u64, DecodeError> {
        let b = *bytes.get(c).ok_or_else(short)?;
        c += 1;
        Ok(b as u64)
    };
    let mut lastpix: u64 = 0;
    for _ in 0..bytepix {
        lastpix = (lastpix << 8) | next()?;
    }
    let mut b = next()?;
    let mut nbits: i32 = 8;
    let mut out = Vec::with_capacity(pixels);
    let mut i = 0;
    while i < pixels {
        nbits -= fsbits;
        while nbits < 0 {
            b = (b << 8) | next()?;
            nbits += 8;
        }
        let fs = (b >> nbits) as i64 - 1;
        b &= (1u64 << nbits) - 1;
        let imax = (i + block).min(pixels);
        if fs < 0 {
            // 差がすべて 0 のブロック。
            while i < imax {
                out.push(lastpix);
                i += 1;
            }
        } else if fs == fsmax {
            // 差をそのまま bbits ずつ書いたブロック。
            while i < imax {
                let mut k = bbits - nbits;
                let mut diff = b << k;
                k -= 8;
                while k >= 0 {
                    b = next()?;
                    diff |= b << k;
                    k -= 8;
                }
                if nbits > 0 {
                    b = next()?;
                    diff |= b >> (-k);
                    b &= (1u64 << nbits) - 1;
                } else {
                    b = 0;
                }
                lastpix = undo(diff & mask, lastpix, mask);
                out.push(lastpix);
                i += 1;
            }
        } else {
            let fs = fs as i32;
            while i < imax {
                while b == 0 {
                    nbits += 8;
                    b = next()?;
                }
                let nzero = nbits - (64 - b.leading_zeros() as i32);
                nbits -= nzero + 1;
                b ^= 1u64 << nbits;
                nbits -= fs;
                while nbits < 0 {
                    b = (b << 8) | next()?;
                    nbits += 8;
                }
                let diff = ((nzero as u64) << fs) | (b >> nbits);
                b &= (1u64 << nbits) - 1;
                lastpix = undo(diff & mask, lastpix, mask);
                out.push(lastpix);
                i += 1;
            }
        }
    }
    Ok(out
        .into_iter()
        .map(|v| match bytepix {
            1 => v as i64,
            2 => v as u16 as i16 as i64,
            _ => v as u32 as i32 as i64,
        })
        .collect())
}

/// 差の符号の折り返しを戻し、前の値に足す（`bytepix` の幅で桁あふれさせる）。
fn undo(diff: u64, lastpix: u64, mask: u64) -> u64 {
    let diff = if diff & 1 == 0 {
        diff >> 1
    } else {
        !(diff >> 1) & mask
    };
    diff.wrapping_add(lastpix) & mask
}

/// 実数を整数に量子化したときの、ディザの乱数（CFITSIO の fits_init_randoms と同じ並び）。
struct Dither {
    method: DitherMethod,
    seed: i64,
}

#[derive(PartialEq)]
enum DitherMethod {
    None,
    Subtractive1,
    Subtractive2,
}

const N_RANDOM: usize = 10000;

fn randoms() -> &'static [f32; N_RANDOM] {
    static VALUES: std::sync::OnceLock<[f32; N_RANDOM]> = std::sync::OnceLock::new();
    VALUES.get_or_init(|| {
        let (a, m) = (16807.0f64, 2147483647.0f64);
        let mut seed = 1.0f64;
        let mut out = [0f32; N_RANDOM];
        for v in out.iter_mut() {
            let temp = a * seed;
            seed = temp - m * (temp / m).trunc();
            *v = (seed / m) as f32;
        }
        out
    })
}

impl Dither {
    fn of(hdu: &Hdu) -> Self {
        let method = match hdu.get("ZQUANTIZ").map(|v| v.trim().to_ascii_uppercase()) {
            Some(v) if v == "NO_DITHER" => DitherMethod::None,
            Some(v) if v == "SUBTRACTIVE_DITHER_2" => DitherMethod::Subtractive2,
            Some(v) if v == "SUBTRACTIVE_DITHER_1" => DitherMethod::Subtractive1,
            // ZQUANTIZ のない古いファイルはディザなし。
            _ => DitherMethod::None,
        };
        Self {
            method,
            seed: hdu.int("ZDITHER0").unwrap_or(1),
        }
    }

    /// `row` 番目（0 始まり）の tile の整数を実数に戻す（CFITSIO の unquantize_i4r4）。
    fn unquantize(
        &self,
        ints: &[i64],
        row: usize,
        scale: f64,
        zero: f64,
        blank: Option<i64>,
    ) -> Vec<f32> {
        let r = randoms();
        let mut iseed = ((row as i64 + self.seed - 1).rem_euclid(N_RANDOM as i64)) as usize;
        let mut next = (r[iseed] * 500.0) as usize;
        ints.iter()
            .map(|&v| {
                let value = if Some(v) == blank {
                    f32::NAN
                } else if self.method == DitherMethod::None {
                    (v as f64 * scale + zero) as f32
                } else if self.method == DitherMethod::Subtractive2 && v == -2147483647 {
                    0.0
                } else {
                    ((v as f64 - r[next] as f64 + 0.5) * scale + zero) as f32
                };
                if self.method != DitherMethod::None {
                    next += 1;
                    if next == N_RANDOM {
                        iseed = (iseed + 1) % N_RANDOM;
                        next = (r[iseed] * 500.0) as usize;
                    }
                }
                value
            })
            .collect()
    }
}
