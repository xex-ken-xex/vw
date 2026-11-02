//! [`Contents`] を NetCDF-3 の CDF-5（64bit の長さと符号なしの型のある形）のバイト列に書く。

use std::collections::HashMap;

use super::{Contents, Kind, Values};

const NC_DIMENSION: u32 = 0x0A;
const NC_VARIABLE: u32 = 0x0B;
const NC_ATTRIBUTE: u32 = 0x0C;

fn nc_type(kind: Kind) -> u32 {
    match kind {
        Kind::I8 => 1,
        Kind::Char => 2,
        Kind::I16 => 3,
        Kind::I32 => 4,
        Kind::F32 => 5,
        Kind::F64 => 6,
        Kind::U8 => 7,
        Kind::U16 => 8,
        Kind::U32 => 9,
        Kind::I64 => 10,
        Kind::U64 => 11,
    }
}

fn pad(out: &mut Vec<u8>) {
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

fn count(out: &mut Vec<u8>, n: usize) {
    out.extend_from_slice(&(n as u64).to_be_bytes());
}

fn name(out: &mut Vec<u8>, s: &str) {
    count(out, s.len());
    out.extend_from_slice(s.as_bytes());
    pad(out);
}

fn attributes(out: &mut Vec<u8>, attrs: &[(String, Values)]) {
    if attrs.is_empty() {
        out.extend_from_slice(&[0; 12]);
        return;
    }
    out.extend_from_slice(&NC_ATTRIBUTE.to_be_bytes());
    count(out, attrs.len());
    for (n, v) in attrs {
        name(out, n);
        out.extend_from_slice(&nc_type(v.kind).to_be_bytes());
        count(out, v.bytes.len() / v.kind.size());
        out.extend_from_slice(&v.bytes);
        pad(out);
    }
}

/// 変数の名前を重ならないようにする（HDF-EOS2 では、別の格子に同じ名前の項目がある）。
fn unique(names: &mut HashMap<String, usize>, base: &str) -> String {
    let n = names.entry(base.to_owned()).or_insert(0);
    *n += 1;
    if *n == 1 {
        base.to_owned()
    } else {
        format!("{base}_{n}")
    }
}

pub fn write(contents: &Contents) -> Vec<u8> {
    // 次元: 同じ名前で大きさの違うものは、名前に大きさを足して分ける。
    let mut dims: Vec<(String, u64)> = Vec::new();
    let mut var_dims: Vec<Vec<usize>> = Vec::new();
    for sds in &contents.datasets {
        let mut ids = Vec::new();
        for (n, s) in &sds.dims {
            let id = match dims.iter().position(|(dn, ds)| dn == n && ds == s) {
                Some(i) => i,
                None => {
                    let name = if dims.iter().any(|(dn, _)| dn == n) {
                        format!("{n}_{s}")
                    } else {
                        n.clone()
                    };
                    match dims.iter().position(|(dn, ds)| *dn == name && ds == s) {
                        Some(i) => i,
                        None => {
                            dims.push((name, *s));
                            dims.len() - 1
                        }
                    }
                }
            };
            ids.push(id);
        }
        var_dims.push(ids);
    }

    let mut header = Vec::new();
    header.extend_from_slice(b"CDF\x05");
    count(&mut header, 0);
    if dims.is_empty() {
        header.extend_from_slice(&[0; 12]);
    } else {
        header.extend_from_slice(&NC_DIMENSION.to_be_bytes());
        count(&mut header, dims.len());
        for (n, s) in &dims {
            name(&mut header, n);
            count(&mut header, *s as usize);
        }
    }
    attributes(&mut header, &contents.attrs);

    // 変数の見出し。データの始まり（begin）は見出しの長さが決まってから書くので、場所を覚える。
    let mut names = HashMap::new();
    let mut begins = Vec::new();
    if contents.datasets.is_empty() {
        header.extend_from_slice(&[0; 12]);
    } else {
        header.extend_from_slice(&NC_VARIABLE.to_be_bytes());
        count(&mut header, contents.datasets.len());
        for (sds, ids) in contents.datasets.iter().zip(&var_dims) {
            name(&mut header, &unique(&mut names, &sds.name));
            count(&mut header, ids.len());
            for &id in ids {
                count(&mut header, id);
            }
            attributes(&mut header, &sds.attrs);
            header.extend_from_slice(&nc_type(sds.data.kind).to_be_bytes());
            let vsize = sds.data.bytes.len().next_multiple_of(4);
            count(&mut header, vsize);
            begins.push(header.len());
            count(&mut header, 0);
        }
    }
    let mut out = header;
    for (sds, at) in contents.datasets.iter().zip(begins) {
        let begin = out.len() as u64;
        out[at..at + 8].copy_from_slice(&begin.to_be_bytes());
        out.extend_from_slice(&sds.data.bytes);
        pad(&mut out);
    }
    out
}
