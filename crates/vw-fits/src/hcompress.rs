//! HCOMPRESS_1 の tile の展開（H 変換の逆と、4 分木で符号化したビット面の復号）。
//!
//! 手順は CFITSIO の fits_hdecompress（hdecompress.c、R. White の hcompress）と同じにしてある。
//! 告知は tile.rs の頭に載せたもの（CFITSIO）と同じ。値は 64bit の整数で持つ。CFITSIO も
//! 32bit の画像では 64bit の版（fits_hdecompress64）を使い、16bit 以下でも結果は同じになるため。

use vw_core::DecodeError;

fn invalid(what: &str) -> DecodeError {
    DecodeError::Invalid(format!("FITS HCOMPRESS tile: {what}"))
}

/// 1 つの tile。`smooth` は ZVAL の SMOOTH（0 でなければ、ほどくときに係数をならす）。
/// 値は 1 次元目（tile の幅）が速く回る順で、数は `pixels` 個でなければ壊れたものとする。
pub fn decode(input: &[u8], smooth: bool, pixels: usize) -> Result<Vec<i64>, DecodeError> {
    let mut bits = Bits::new(input);
    if bits.bytes(2)? != [0xDD, 0x99] {
        return Err(invalid("bad magic number"));
    }
    // 符号化の側は 2 次元目を nx、1 次元目を ny と呼ぶ（a[nx][ny] で ny が速く回る）。
    let nx = bits.int(4)? as usize;
    let ny = bits.int(4)? as usize;
    let scale = bits.int(4)?;
    let sum_all = bits.int(8)?;
    let planes = bits.bytes(3)?;
    let nel = nx
        .checked_mul(ny)
        .filter(|&n| n == pixels && n > 0)
        .ok_or_else(|| invalid("size does not match the tile"))?;
    let mut a = vec![0i64; nel];
    let (nx2, ny2) = (nx.div_ceil(2), ny.div_ceil(2));
    bits.restart();
    let quadrants = [
        (0, nx2, ny2, planes[0]),
        (ny2, nx2, ny / 2, planes[1]),
        (ny * nx2, nx / 2, ny2, planes[1]),
        (ny * nx2 + ny2, nx / 2, ny / 2, planes[2]),
    ];
    for (start, qx, qy, nplanes) in quadrants {
        qtree_decode(&mut bits, &mut a[start..], ny, qx, qy, nplanes.into())?;
    }
    if bits.nybble()? != 0 {
        return Err(invalid("bad bit plane values"));
    }
    bits.restart();
    for v in a.iter_mut() {
        if *v != 0 && bits.bit()? != 0 {
            *v = -*v;
        }
    }
    a[0] = sum_all;
    if scale > 1 {
        for v in a.iter_mut() {
            *v = v.wrapping_mul(scale);
        }
    }
    h_inverse(&mut a, nx, ny, smooth, scale);
    Ok(a)
}

/// 1 つの象限（`nqx` × `nqy`、行の幅は `n`）のビット面を、上のビットから読んで `a` に足す。
fn qtree_decode(
    bits: &mut Bits,
    a: &mut [i64],
    n: usize,
    nqx: usize,
    nqy: usize,
    nbitplanes: u32,
) -> Result<(), DecodeError> {
    let log2n = ceil_log2(nqx.max(nqy));
    let mut scratch = vec![0u8; nqx.div_ceil(2) * nqy.div_ceil(2)];
    for bit in (0..nbitplanes).rev() {
        if bit >= 63 {
            return Err(invalid("too many bit planes"));
        }
        match bits.nybble()? {
            // ビット面をそのまま、4 画素を 1 つの 4 ビットにして書いたもの。
            0 => {
                for s in scratch.iter_mut() {
                    *s = bits.nybble()?;
                }
            }
            0xF => {
                let mut current = vec![bits.huffman()?];
                let (mut nx, mut ny) = (1usize, 1usize);
                let (mut nfx, mut nfy) = (nqx, nqy);
                let mut c = 1usize << log2n;
                // 大きさの並び n[k-1] = (n[k] + 1) / 2（n[log2n] が nqx・nqy）を、小さい方から作る。
                for _ in 1..log2n {
                    c >>= 1;
                    nx <<= 1;
                    ny <<= 1;
                    if nfx <= c {
                        nx -= 1;
                    } else {
                        nfx -= c;
                    }
                    if nfy <= c {
                        ny -= 1;
                    } else {
                        nfy -= c;
                    }
                    let mut next = vec![0u8; nx * ny];
                    expand(&current, nx, ny, |i, v| next[i] = v);
                    for v in next.iter_mut().rev() {
                        if *v != 0 {
                            *v = bits.huffman()?;
                        }
                    }
                    current = next;
                }
                scratch = current;
                if log2n == 0 {
                    // 1 x 1 の象限は、最初の符号がそのまま 4 画素分の値。
                    scratch.resize(1, 0);
                }
            }
            _ => return Err(invalid("bad quadtree format code")),
        }
        let need = nqx.div_ceil(2) * nqy.div_ceil(2);
        if scratch.len() < need {
            return Err(invalid("quadtree is smaller than its quadrant"));
        }
        expand(&scratch, nqx, nqy, |i, v| {
            let (x, y) = (i / nqy, i % nqy);
            a[x * n + y] |= i64::from(v) << bit;
        });
    }
    Ok(())
}

/// 4 ビットの値の並び（`(nx+1)/2` × `(ny+1)/2`）を、1 つの値を 2 × 2 の 0・1 に広げて `nx` × `ny` にする。
/// 2 × 2 の中は、左上が 8、右上が 4、左下が 2、右下が 1 のビット（a[i][j] の i が縦）。
fn expand(values: &[u8], nx: usize, ny: usize, mut put: impl FnMut(usize, u8)) {
    let ny2 = ny.div_ceil(2);
    for i in 0..nx {
        for j in 0..ny {
            let v = values.get((i / 2) * ny2 + j / 2).copied().unwrap_or(0);
            let shift = 3 - 2 * (i % 2) - (j % 2);
            put(i * ny + j, (v >> shift) & 1);
        }
    }
}

fn ceil_log2(n: usize) -> u32 {
    if n <= 1 {
        0
    } else {
        usize::BITS - (n - 1).leading_zeros()
    }
}

/// H 変換の逆。係数を 2 × 2 の画素に戻すことを、粗い段から log2n 回繰り返す。
fn h_inverse(a: &mut [i64], nx: usize, ny: usize, smooth: bool, scale: i64) {
    let log2n = ceil_log2(nx.max(ny));
    if log2n == 0 {
        return;
    }
    let mut tmp = vec![0i64; nx.max(ny).div_ceil(2)];
    let mut shift = 1;
    let mut bit0: i64 = 1 << (log2n - 1);
    let mut bit1 = bit0 << 1;
    let mut bit2 = bit0 << 2;
    let mut mask0 = -bit0;
    let mut mask1 = mask0 << 1;
    let mask2 = mask0 << 2;
    let mut prnd0 = bit0 >> 1;
    let mut prnd1 = bit1 >> 1;
    let prnd2 = bit2 >> 1;
    let mut nrnd0 = prnd0 - 1;
    let mut nrnd1 = prnd1 - 1;
    let nrnd2 = prnd2 - 1;
    let round = |v: i64, p: i64, n: i64, m: i64| (v + if v >= 0 { p } else { n }) & m;
    a[0] = round(a[0], prnd2, nrnd2, mask2);
    let (mut nxtop, mut nytop) = (1usize, 1usize);
    let (mut nxf, mut nyf) = (nx, ny);
    let mut c = 1usize << log2n;
    for k in (0..log2n).rev() {
        c >>= 1;
        nxtop <<= 1;
        nytop <<= 1;
        if nxf <= c {
            nxtop -= 1;
        } else {
            nxf -= c;
        }
        if nyf <= c {
            nytop -= 1;
        } else {
            nyf -= c;
        }
        // 最後の段は 4 で割る（係数の和の重みが 2 段分）。
        if k == 0 {
            nrnd0 = 0;
            shift = 2;
        }
        for i in 0..nxtop {
            interleave(&mut a[ny * i..], nytop, 1, &mut tmp);
        }
        for j in 0..nytop {
            interleave(&mut a[j..], nxtop, ny, &mut tmp);
        }
        if smooth {
            h_smooth(a, nxtop, nytop, ny, scale);
        }
        let (oddx, oddy) = (nxtop % 2, nytop % 2);
        let mut i = 0;
        while i + oddx < nxtop {
            let mut s00 = ny * i;
            let mut s10 = s00 + ny;
            let mut j = 0;
            while j + oddy < nytop {
                let h0 = a[s00];
                let hx = round(a[s10], prnd1, nrnd1, mask1);
                let hy = round(a[s00 + 1], prnd1, nrnd1, mask1);
                let hc = round(a[s10 + 1], prnd0, nrnd0, mask0);
                let lowbit0 = hc & bit0;
                let hx = if hx >= 0 { hx - lowbit0 } else { hx + lowbit0 };
                let hy = if hy >= 0 { hy - lowbit0 } else { hy + lowbit0 };
                let lowbit1 = (hc ^ hx ^ hy) & bit1;
                let h0 = if h0 >= 0 {
                    h0 + lowbit0 - lowbit1
                } else if lowbit0 == 0 {
                    h0 + lowbit1
                } else {
                    h0 + lowbit0 - lowbit1
                };
                a[s10 + 1] = (h0 + hx + hy + hc) >> shift;
                a[s10] = (h0 + hx - hy - hc) >> shift;
                a[s00 + 1] = (h0 - hx + hy - hc) >> shift;
                a[s00] = (h0 - hx - hy + hc) >> shift;
                s00 += 2;
                s10 += 2;
                j += 2;
            }
            if oddy == 1 {
                let h0 = a[s00];
                let hx = round(a[s10], prnd1, nrnd1, mask1);
                let lowbit1 = hx & bit1;
                let h0 = if h0 >= 0 { h0 - lowbit1 } else { h0 + lowbit1 };
                a[s10] = (h0 + hx) >> shift;
                a[s00] = (h0 - hx) >> shift;
            }
            i += 2;
        }
        if oddx == 1 {
            let mut s00 = ny * i;
            let mut j = 0;
            while j + oddy < nytop {
                let h0 = a[s00];
                let hy = round(a[s00 + 1], prnd1, nrnd1, mask1);
                let lowbit1 = hy & bit1;
                let h0 = if h0 >= 0 { h0 - lowbit1 } else { h0 + lowbit1 };
                a[s00 + 1] = (h0 + hy) >> shift;
                a[s00] = (h0 - hy) >> shift;
                s00 += 2;
                j += 2;
            }
            if oddy == 1 {
                a[s00] >>= shift;
            }
        }
        bit2 = bit1;
        bit1 = bit0;
        bit0 >>= 1;
        mask1 = mask0;
        mask0 >>= 1;
        prnd1 = prnd0;
        prnd0 >>= 1;
        nrnd1 = nrnd0;
        nrnd0 = prnd0 - 1;
    }
    let _ = bit2;
}

/// 間隔 `stride` で並ぶ `n` 個の値の、前半を偶数番目に、後半を奇数番目に並べ直す。
fn interleave(a: &mut [i64], n: usize, stride: usize, tmp: &mut [i64]) {
    let half = n.div_ceil(2);
    for (t, i) in tmp.iter_mut().zip(half..n) {
        *t = a[stride * i];
    }
    for i in (0..half).rev() {
        a[2 * i * stride] = a[i * stride];
    }
    for (k, i) in (1..n).step_by(2).enumerate() {
        a[i * stride] = tmp[k];
    }
}

/// 係数をならす（SMOOTH）。となりの区画の平均とつながるように傾きと曲がりを直し、直す幅は
/// 量子化の幅の半分（scale / 2）までにする。区画の端の係数は直さない。
fn h_smooth(a: &mut [i64], nxtop: usize, nytop: usize, ny: usize, scale: i64) {
    let smax = scale >> 1;
    if smax <= 0 {
        return;
    }
    let ny2 = ny << 1;
    let limit = |s: i64| s.clamp(-smax, smax);
    // 横の差 hx
    let mut i = 2;
    while i + 2 < nxtop {
        let mut s00 = ny * i;
        let mut s10 = s00 + ny;
        let mut j = 0;
        while j < nytop {
            let (hm, h0, hp) = (a[s00 - ny2], a[s00], a[s00 + ny2]);
            let dmax = ((hp - h0).min(h0 - hm)).max(0) << 2;
            let dmin = ((hp - h0).max(h0 - hm)).min(0) << 2;
            if dmin < dmax {
                let diff = (hp - hm).min(dmax).max(dmin);
                let s = diff - (a[s10] << 3);
                let s = if s >= 0 { s >> 3 } else { (s + 7) >> 3 };
                a[s10] += limit(s);
            }
            s00 += 2;
            s10 += 2;
            j += 2;
        }
        i += 2;
    }
    // 縦の差 hy
    let mut i = 0;
    while i < nxtop {
        let mut s00 = ny * i + 2;
        let mut j = 2;
        while j + 2 < nytop {
            let (hm, h0, hp) = (a[s00 - 2], a[s00], a[s00 + 2]);
            let dmax = ((hp - h0).min(h0 - hm)).max(0) << 2;
            let dmin = ((hp - h0).max(h0 - hm)).min(0) << 2;
            if dmin < dmax {
                let diff = (hp - hm).min(dmax).max(dmin);
                let s = diff - (a[s00 + 1] << 3);
                let s = if s >= 0 { s >> 3 } else { (s + 7) >> 3 };
                a[s00 + 1] += limit(s);
            }
            s00 += 2;
            j += 2;
        }
        i += 2;
    }
    // 曲がり hc
    let mut i = 2;
    while i + 2 < nxtop {
        let mut s00 = ny * i + 2;
        let mut s10 = s00 + ny;
        let mut j = 2;
        while j + 2 < nytop {
            let hmm = a[s00 - ny2 - 2];
            let hpm = a[s00 + ny2 - 2];
            let hmp = a[s00 - ny2 + 2];
            let hpp = a[s00 + ny2 + 2];
            let h0 = a[s00];
            let diff = hpp + hmm - hmp - hpm;
            let hx2 = a[s10] << 1;
            let hy2 = a[s00 + 1] << 1;
            let m1 = ((hpp - h0).max(0) - hx2 - hy2).min((h0 - hpm).max(0) + hx2 - hy2);
            let m2 = ((h0 - hmp).max(0) - hx2 + hy2).min((hmm - h0).max(0) + hx2 + hy2);
            let dmax = m1.min(m2) << 4;
            let m1 = ((hpp - h0).min(0) - hx2 - hy2).max((h0 - hpm).min(0) + hx2 - hy2);
            let m2 = ((h0 - hmp).min(0) - hx2 + hy2).max((hmm - h0).min(0) + hx2 + hy2);
            let dmin = m1.max(m2) << 4;
            if dmin < dmax {
                let diff = diff.min(dmax).max(dmin);
                let s = diff - (a[s10 + 1] << 6);
                let s = if s >= 0 { s >> 6 } else { (s + 63) >> 6 };
                a[s10 + 1] += limit(s);
            }
            s00 += 2;
            s10 += 2;
            j += 2;
        }
        i += 2;
    }
}

/// 上位ビットから読む。`restart` で読みかけのバイトを捨てて、次のバイトの頭から読み直す。
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
    buffer: u32,
    left: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            at: 0,
            buffer: 0,
            left: 0,
        }
    }

    fn byte(&mut self) -> Result<u8, DecodeError> {
        let b = *self
            .data
            .get(self.at)
            .ok_or_else(|| invalid("data ends early"))?;
        self.at += 1;
        Ok(b)
    }

    fn bytes(&mut self, n: usize) -> Result<Vec<u8>, DecodeError> {
        (0..n).map(|_| self.byte()).collect()
    }

    /// `n` バイトの、上位バイトが先の符号付き整数。
    fn int(&mut self, n: usize) -> Result<i64, DecodeError> {
        let mut v: i64 = 0;
        for _ in 0..n {
            v = (v << 8) | i64::from(self.byte()?);
        }
        let shift = 64 - 8 * n as u32;
        Ok((v << shift) >> shift)
    }

    fn restart(&mut self) {
        self.buffer = 0;
        self.left = 0;
    }

    fn bits(&mut self, n: u32) -> Result<u8, DecodeError> {
        if self.left < n {
            self.buffer = (self.buffer << 8) | u32::from(self.byte()?);
            self.left += 8;
        }
        self.left -= n;
        Ok(((self.buffer >> self.left) & ((1 << n) - 1)) as u8)
    }

    fn bit(&mut self) -> Result<u8, DecodeError> {
        self.bits(1)
    }

    fn nybble(&mut self) -> Result<u8, DecodeError> {
        self.bits(4)
    }

    /// 4 ビットの値のハフマン符号。
    fn huffman(&mut self) -> Result<u8, DecodeError> {
        let c = self.bits(3)?;
        if c < 4 {
            return Ok(1 << c);
        }
        let c = (c << 1) | self.bit()?;
        match c {
            8 => return Ok(3),
            9 => return Ok(5),
            10 => return Ok(10),
            11 => return Ok(12),
            12 => return Ok(15),
            _ => {}
        }
        let c = (c << 1) | self.bit()?;
        match c {
            26 => return Ok(6),
            27 => return Ok(7),
            28 => return Ok(9),
            29 => return Ok(11),
            30 => return Ok(13),
            _ => {}
        }
        let c = (c << 1) | self.bit()?;
        Ok(if c == 62 { 0 } else { 14 })
    }
}
