//! HDF4 の圧縮をほどく。deflate はもう使っている hdf5-reader の関数を使い、RLE と Huffman は
//! 自分で書く。どちらも数十行で、外のクレートを足すほどではないため。

/// 圧縮の種類（HDF4 の comp_type）。
pub const NONE: u16 = 0;
pub const RLE: u16 = 1;
pub const NBIT: u16 = 2;
pub const SKPHUFF: u16 = 3;
pub const DEFLATE: u16 = 4;
pub const SZIP: u16 = 5;

/// `comp_type` の圧縮をほどく。`info` は特別な要素の見出しの、圧縮ごとの情報。`length` はほどいた後の長さ。
pub fn decompress(
    comp_type: u16,
    info: &[u8],
    input: &[u8],
    length: usize,
) -> Result<Vec<u8>, String> {
    let out = match comp_type {
        NONE => input.to_vec(),
        RLE => rle(input, length)?,
        SKPHUFF => {
            let skip = info
                .get(..4)
                .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
                .ok_or("HDF4: short Huffman header")?;
            skipping_huffman(input, length, skip.max(1))?
        }
        DEFLATE => hdf5_reader::filters::deflate::decompress_with_limit(input, length)
            .map_err(|e| format!("HDF4: {e}"))?,
        // szip は圧縮する側のない環境でテストのデータを作れず、確かめられないので、まだ読まない。
        NBIT => return Err("HDF4: N-bit compression is not supported".into()),
        SZIP => return Err("HDF4: szip compression is not supported yet".into()),
        other => return Err(format!("HDF4: compression {other} is not supported")),
    };
    if out.len() < length {
        return Err(format!(
            "HDF4: {} bytes after decompression, expected {length}",
            out.len()
        ));
    }
    Ok(out)
}

/// 同じバイトの並びの長さと、そのままの並びを交互に書く RLE（HDF4 の crle.c）。
/// 先頭のバイトの上位ビットが立っていれば、(下位 7 ビット + 3) 個の同じバイト。立っていなければ、
/// (そのバイト + 1) 個をそのまま。
pub fn rle(input: &[u8], length: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(length);
    let mut i = 0;
    let short = || "HDF4: RLE data ends early".to_owned();
    while out.len() < length {
        let c = *input.get(i).ok_or_else(short)?;
        i += 1;
        if c & 0x80 != 0 {
            let n = usize::from(c & 0x7f) + 3;
            let b = *input.get(i).ok_or_else(short)?;
            i += 1;
            out.extend(std::iter::repeat_n(b, n));
        } else {
            let n = usize::from(c) + 1;
            out.extend_from_slice(input.get(i..i + n).ok_or_else(short)?);
            i += n;
        }
    }
    out.truncate(length);
    Ok(out)
}

/// 符号の木の葉の数（バイトの値）と、節の数。
const LEAVES: usize = 256;
const NODES: usize = 2 * LEAVES + 1;

/// 1 つの適応ハフマンの木（伸ばし木、HDF4 の cskphuff.c）。
struct Splay {
    left: Vec<usize>,
    right: Vec<usize>,
    up: Vec<usize>,
}

impl Splay {
    fn new() -> Self {
        Self {
            left: (0..LEAVES).map(|i| 2 * i).collect(),
            right: (0..LEAVES).map(|i| 2 * i + 1).collect(),
            up: (0..NODES).map(|i| i / 2).collect(),
        }
    }

    /// 読んだ葉を根の近くへ寄せる。
    fn splay(&mut self, plain: usize) {
        let mut a = plain + LEAVES;
        loop {
            let c = self.up[a];
            if c == 0 {
                break;
            }
            let d = self.up[c];
            let mut b = self.left[d];
            if c == b {
                b = self.right[d];
                self.right[d] = a;
            } else {
                self.left[d] = a;
            }
            if a == self.left[c] {
                self.left[c] = b;
            } else {
                self.right[c] = b;
            }
            self.up[a] = d;
            self.up[b] = c;
            a = d;
            if a == 0 {
                break;
            }
        }
    }
}

/// 「飛ばし」付きのハフマン（HDF4 の cskphuff.c）。`skip` バイトごとに別の木を使う
/// （多バイトの数の、上位のバイトと下位のバイトを別々に符号にする）。
pub fn skipping_huffman(input: &[u8], length: usize, skip: usize) -> Result<Vec<u8>, String> {
    let mut trees: Vec<Splay> = (0..skip).map(|_| Splay::new()).collect();
    let mut out = Vec::with_capacity(length);
    let mut bit = 0usize;
    let total_bits = input.len() * 8;
    for k in 0..length {
        let tree = &mut trees[k % skip];
        let mut a = 0usize;
        while a < LEAVES {
            if bit >= total_bits {
                return Err("HDF4: Huffman data ends early".into());
            }
            let set = input[bit / 8] & (0x80 >> (bit % 8)) != 0;
            bit += 1;
            a = if set { tree.right[a] } else { tree.left[a] };
        }
        let plain = a - LEAVES;
        tree.splay(plain);
        out.push(plain as u8);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rle_repeats_runs_and_copies_literals() {
        // 0x82 → 同じバイト 5 個、0x01 → そのまま 2 個。
        assert_eq!(
            rle(&[0x82, 7, 0x01, 1, 2], 7).unwrap(),
            [7, 7, 7, 7, 7, 1, 2]
        );
    }

    #[test]
    fn rle_that_ends_early_is_rejected() {
        assert!(rle(&[0x05, 1], 6).is_err());
    }
}
