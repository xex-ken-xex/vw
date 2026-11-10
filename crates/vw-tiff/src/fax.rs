//! 白黒の FAX 圧縮（CCITT の Modified Huffman と T.4）をほどく。T.6（Group 4）は tiff クレートが読む。
//!
//! tiff 経由で入っている fax クレートを使わないのは、T.4 の 1 次元しか読めず、行ごとに
//! 1 次元と 2 次元が混ざる T.4 の 2 次元や、行の頭をバイトにそろえる Modified Huffman を読む口がないため。
//! 符号の表は ITU-T T.4 の表 1〜3。

use std::sync::OnceLock;

use vw_core::DecodeError;

/// TIFF の Compression の値で決まる、行の符号の形。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// 2: 1 次元の符号だけ。EOL はなく、各行の頭はバイトの境目。
    Huffman,
    /// 32771: [`Kind::Huffman`] と同じで、各行の頭は 2 バイトの境目。
    HuffmanWord,
    /// 3: 各行の前に EOL。`two_d` なら EOL の後の 1 ビットで、その行が 1 次元か 2 次元かを示す。
    T4 { two_d: bool },
}

impl Kind {
    /// Compression と T4Options（タグ 292）から。FAX の圧縮でなければ `None`。
    pub fn of(compression: u64, t4_options: u64) -> Option<Self> {
        match compression {
            2 => Some(Self::Huffman),
            32771 => Some(Self::HuffmanWord),
            3 => Some(Self::T4 {
                two_d: t4_options & 1 != 0,
            }),
            _ => None,
        }
    }
}

/// 1 つのストリップかタイル（幅 `width`、`rows` 行）をほどき、1 画素 1 ビット（黒が 1、白が 0）を
/// 上位ビットから詰めた行の並びにする。行末は 1 バイトに切り上げる。`lsb_first` は FillOrder = 2。
///
/// 白を 0 にするのは libtiff と同じで、白黒のどちらを明るく見せるかは PhotometricInterpretation に任せる。
pub fn decode(
    input: &[u8],
    width: usize,
    rows: usize,
    kind: Kind,
    lsb_first: bool,
) -> Result<Vec<u8>, DecodeError> {
    let reversed: Vec<u8>;
    let input = if lsb_first {
        reversed = input.iter().map(|b| b.reverse_bits()).collect();
        &reversed[..]
    } else {
        input
    };
    let row_bytes = width.div_ceil(8);
    let mut out = vec![0u8; row_bytes * rows];
    let mut bits = Bits::new(input);
    let mut reference: Vec<usize> = Vec::new();
    let mut current = Vec::new();
    for row in 0..rows {
        let two_d = match kind {
            Kind::Huffman => {
                bits.align(8);
                false
            }
            Kind::HuffmanWord => {
                bits.align(16);
                false
            }
            Kind::T4 { two_d } => {
                bits.skip_eol();
                two_d && bits.read(1) == 0
            }
        };
        current.clear();
        if two_d {
            read_2d(&mut bits, &reference, width, &mut current)?;
        } else {
            read_1d(&mut bits, width, &mut current)?;
        }
        if bits.past_end() {
            return Err(broken("data ends before the last row"));
        }
        fill_black(
            &current,
            width,
            &mut out[row * row_bytes..(row + 1) * row_bytes],
        );
        std::mem::swap(&mut reference, &mut current);
    }
    Ok(out)
}

fn broken(what: &str) -> DecodeError {
    DecodeError::Invalid(format!("broken CCITT fax data: {what}"))
}

/// 1 次元の行。白から始めて交互の長さを読み、色の変わる位置を `out` に並べる。
fn read_1d(bits: &mut Bits, width: usize, out: &mut Vec<usize>) -> Result<(), DecodeError> {
    let mut at = 0;
    let mut black = false;
    while at < width {
        at += run(bits, black)?;
        if at > width {
            return Err(broken("a run goes past the row"));
        }
        out.push(at);
        black = !black;
    }
    Ok(())
}

/// 2 次元の行。上の行（`reference` の色の変わる位置）との違いで読む。
fn read_2d(
    bits: &mut Bits,
    reference: &[usize],
    width: usize,
    out: &mut Vec<usize>,
) -> Result<(), DecodeError> {
    // a0 は行の頭の手前（-1）から始まる。そのときだけ b1 は 0 の位置も含む。
    let mut a0: Option<usize> = None;
    let mut black = false;
    while a0.is_none_or(|a| a < width) {
        let (b1, b2) = b1_b2(reference, a0, black, width);
        match mode(bits)? {
            Mode::Pass => a0 = Some(b2),
            Mode::Horizontal => {
                let start = a0.unwrap_or(0);
                let a1 = start + run(bits, black)?;
                let a2 = a1 + run(bits, !black)?;
                if a2 > width {
                    return Err(broken("a run goes past the row"));
                }
                out.extend([a1, a2]);
                a0 = Some(a2);
            }
            Mode::Vertical(d) => {
                let a1 = b1
                    .checked_add_signed(d)
                    .filter(|&a| a <= width && a0.is_none_or(|a0| a >= a0))
                    .ok_or_else(|| broken("a vertical code points outside the row"))?;
                out.push(a1);
                a0 = Some(a1);
                black = !black;
            }
        }
    }
    Ok(())
}

/// 上の行で、a0 より右にあって今の色から反対の色に変わる最初の位置（b1）と、その次の変わり目（b2）。
/// なければ行の幅。
fn b1_b2(reference: &[usize], a0: Option<usize>, black: bool, width: usize) -> (usize, usize) {
    // 変わり目は白→黒、黒→白の順に並ぶので、偶数番目が黒に変わる位置。
    let wanted = usize::from(black);
    let i = reference
        .iter()
        .enumerate()
        .position(|(i, &p)| i % 2 == wanted && a0.is_none_or(|a| p > a))
        .unwrap_or(reference.len());
    let at = |i: usize| reference.get(i).copied().unwrap_or(width).min(width);
    (at(i), at(i + 1))
}

/// 変わり目の並び（白から始まる）から、黒の画素を 1 にする。
fn fill_black(changes: &[usize], width: usize, row: &mut [u8]) {
    for pair in changes.chunks(2) {
        let start = pair[0];
        let end = pair.get(1).copied().unwrap_or(width).min(width);
        for x in start..end {
            row[x / 8] |= 0x80 >> (x % 8);
        }
    }
}

/// 1 つの色の長さ。64 以上の長さは「まとめの符号」の後に 64 未満の符号が続く。
fn run(bits: &mut Bits, black: bool) -> Result<usize, DecodeError> {
    let table = if black {
        &tables().black
    } else {
        &tables().white
    };
    let mut total = 0usize;
    loop {
        let (len, value) = table[bits.peek(LOOKUP_BITS) as usize];
        if len == 0 {
            return Err(broken("unknown run-length code"));
        }
        bits.consume(len);
        total += usize::from(value);
        if value < 64 {
            return Ok(total);
        }
    }
}

enum Mode {
    Pass,
    Horizontal,
    Vertical(isize),
}

fn mode(bits: &mut Bits) -> Result<Mode, DecodeError> {
    const MODES: &[(&str, i8)] = &[
        ("1", 0),
        ("011", 1),
        ("010", -1),
        ("001", HORIZONTAL),
        ("0001", PASS),
        ("000011", 2),
        ("000010", -2),
        ("0000011", 3),
        ("0000010", -3),
    ];
    const HORIZONTAL: i8 = 100;
    const PASS: i8 = 101;
    let peek = bits.peek(7);
    for &(code, value) in MODES {
        let len = code.len() as u32;
        if peek >> (7 - len) == u32::from_str_radix(code, 2).unwrap_or(u32::MAX) {
            bits.consume(len);
            return Ok(match value {
                HORIZONTAL => Mode::Horizontal,
                PASS => Mode::Pass,
                d => Mode::Vertical(isize::from(d)),
            });
        }
    }
    // 0000001 は拡張（非圧縮の形）。対応する書き手がほぼないので読まない。
    Err(broken("unknown or unsupported 2D mode code"))
}

/// 表を引くときに先読みするビット数（一番長い黒の符号が 13 ビット）。
const LOOKUP_BITS: u32 = 13;

struct Tables {
    white: Vec<(u32, u16)>,
    black: Vec<(u32, u16)>,
}

/// 13 ビットの先読みの値から、（符号の長さ, 色の長さ）を引く表。符号でない値は長さ 0。
fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let build = |codes: &[(&str, u16)]| {
            let mut t = vec![(0u32, 0u16); 1 << LOOKUP_BITS];
            for &(code, value) in codes {
                let len = code.len() as u32;
                let bits = u32::from_str_radix(code, 2).unwrap_or(0);
                let first = (bits << (LOOKUP_BITS - len)) as usize;
                for slot in &mut t[first..first + (1 << (LOOKUP_BITS - len))] {
                    *slot = (len, value);
                }
            }
            t
        };
        Tables {
            white: build(WHITE),
            black: build(BLACK),
        }
    })
}

/// 上位ビットから読む。終わりの先は 0 が続くものとして読み、読み過ぎたかを [`Bits::past_end`] で知らせる。
struct Bits<'a> {
    data: &'a [u8],
    /// 次に読むビットの位置。
    at: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    fn peek(&self, n: u32) -> u32 {
        let mut v = 0u32;
        for i in 0..n as usize {
            let bit = self.at + i;
            let b = self
                .data
                .get(bit / 8)
                .map_or(0, |b| (b >> (7 - bit % 8)) & 1);
            v = (v << 1) | u32::from(b);
        }
        v
    }

    fn consume(&mut self, n: u32) {
        self.at += n as usize;
    }

    fn read(&mut self, n: u32) -> u32 {
        let v = self.peek(n);
        self.consume(n);
        v
    }

    fn align(&mut self, bits: usize) {
        self.at = self.at.div_ceil(bits) * bits;
    }

    /// 行の頭の EOL（0 が 11 個以上続いて 1）があれば飛ばす。バイトにそろえるための 0 もここで飛ばす。
    fn skip_eol(&mut self) {
        if self.peek(11) != 0 {
            return;
        }
        while !self.past_end() && self.peek(1) == 0 {
            self.consume(1);
        }
        self.consume(1);
    }

    fn past_end(&self) -> bool {
        self.at > self.data.len() * 8
    }
}

/// 白の長さの符号（T.4 の表 1〜3。1792 以上は白黒で共通）。
const WHITE: &[(&str, u16)] = &[
    ("00110101", 0),
    ("000111", 1),
    ("0111", 2),
    ("1000", 3),
    ("1011", 4),
    ("1100", 5),
    ("1110", 6),
    ("1111", 7),
    ("10011", 8),
    ("10100", 9),
    ("00111", 10),
    ("01000", 11),
    ("001000", 12),
    ("000011", 13),
    ("110100", 14),
    ("110101", 15),
    ("101010", 16),
    ("101011", 17),
    ("0100111", 18),
    ("0001100", 19),
    ("0001000", 20),
    ("0010111", 21),
    ("0000011", 22),
    ("0000100", 23),
    ("0101000", 24),
    ("0101011", 25),
    ("0010011", 26),
    ("0100100", 27),
    ("0011000", 28),
    ("00000010", 29),
    ("00000011", 30),
    ("00011010", 31),
    ("00011011", 32),
    ("00010010", 33),
    ("00010011", 34),
    ("00010100", 35),
    ("00010101", 36),
    ("00010110", 37),
    ("00010111", 38),
    ("00101000", 39),
    ("00101001", 40),
    ("00101010", 41),
    ("00101011", 42),
    ("00101100", 43),
    ("00101101", 44),
    ("00000100", 45),
    ("00000101", 46),
    ("00001010", 47),
    ("00001011", 48),
    ("01010010", 49),
    ("01010011", 50),
    ("01010100", 51),
    ("01010101", 52),
    ("00100100", 53),
    ("00100101", 54),
    ("01011000", 55),
    ("01011001", 56),
    ("01011010", 57),
    ("01011011", 58),
    ("01001010", 59),
    ("01001011", 60),
    ("00110010", 61),
    ("00110011", 62),
    ("00110100", 63),
    ("11011", 64),
    ("10010", 128),
    ("010111", 192),
    ("0110111", 256),
    ("00110110", 320),
    ("00110111", 384),
    ("01100100", 448),
    ("01100101", 512),
    ("01101000", 576),
    ("01100111", 640),
    ("011001100", 704),
    ("011001101", 768),
    ("011010010", 832),
    ("011010011", 896),
    ("011010100", 960),
    ("011010101", 1024),
    ("011010110", 1088),
    ("011010111", 1152),
    ("011011000", 1216),
    ("011011001", 1280),
    ("011011010", 1344),
    ("011011011", 1408),
    ("010011000", 1472),
    ("010011001", 1536),
    ("010011010", 1600),
    ("011000", 1664),
    ("010011011", 1728),
    ("00000001000", 1792),
    ("00000001100", 1856),
    ("00000001101", 1920),
    ("000000010010", 1984),
    ("000000010011", 2048),
    ("000000010100", 2112),
    ("000000010101", 2176),
    ("000000010110", 2240),
    ("000000010111", 2304),
    ("000000011100", 2368),
    ("000000011101", 2432),
    ("000000011110", 2496),
    ("000000011111", 2560),
];
/// 黒の長さの符号（T.4 の表 1〜3。1792 以上は白黒で共通）。
const BLACK: &[(&str, u16)] = &[
    ("0000110111", 0),
    ("010", 1),
    ("11", 2),
    ("10", 3),
    ("011", 4),
    ("0011", 5),
    ("0010", 6),
    ("00011", 7),
    ("000101", 8),
    ("000100", 9),
    ("0000100", 10),
    ("0000101", 11),
    ("0000111", 12),
    ("00000100", 13),
    ("00000111", 14),
    ("000011000", 15),
    ("0000010111", 16),
    ("0000011000", 17),
    ("0000001000", 18),
    ("00001100111", 19),
    ("00001101000", 20),
    ("00001101100", 21),
    ("00000110111", 22),
    ("00000101000", 23),
    ("00000010111", 24),
    ("00000011000", 25),
    ("000011001010", 26),
    ("000011001011", 27),
    ("000011001100", 28),
    ("000011001101", 29),
    ("000001101000", 30),
    ("000001101001", 31),
    ("000001101010", 32),
    ("000001101011", 33),
    ("000011010010", 34),
    ("000011010011", 35),
    ("000011010100", 36),
    ("000011010101", 37),
    ("000011010110", 38),
    ("000011010111", 39),
    ("000001101100", 40),
    ("000001101101", 41),
    ("000011011010", 42),
    ("000011011011", 43),
    ("000001010100", 44),
    ("000001010101", 45),
    ("000001010110", 46),
    ("000001010111", 47),
    ("000001100100", 48),
    ("000001100101", 49),
    ("000001010010", 50),
    ("000001010011", 51),
    ("000000100100", 52),
    ("000000110111", 53),
    ("000000111000", 54),
    ("000000100111", 55),
    ("000000101000", 56),
    ("000001011000", 57),
    ("000001011001", 58),
    ("000000101011", 59),
    ("000000101100", 60),
    ("000001011010", 61),
    ("000001100110", 62),
    ("000001100111", 63),
    ("0000001111", 64),
    ("000011001000", 128),
    ("000011001001", 192),
    ("000001011011", 256),
    ("000000110011", 320),
    ("000000110100", 384),
    ("000000110101", 448),
    ("0000001101100", 512),
    ("0000001101101", 576),
    ("0000001001010", 640),
    ("0000001001011", 704),
    ("0000001001100", 768),
    ("0000001001101", 832),
    ("0000001110010", 896),
    ("0000001110011", 960),
    ("0000001110100", 1024),
    ("0000001110101", 1088),
    ("0000001110110", 1152),
    ("0000001110111", 1216),
    ("0000001010010", 1280),
    ("0000001010011", 1344),
    ("0000001010100", 1408),
    ("0000001010101", 1472),
    ("0000001011010", 1536),
    ("0000001011011", 1600),
    ("0000001100100", 1664),
    ("0000001100101", 1728),
    ("00000001000", 1792),
    ("00000001100", 1856),
    ("00000001101", 1920),
    ("000000010010", 1984),
    ("000000010011", 2048),
    ("000000010100", 2112),
    ("000000010101", 2176),
    ("000000010110", 2240),
    ("000000010111", 2304),
    ("000000011100", 2368),
    ("000000011101", 2432),
    ("000000011110", 2496),
    ("000000011111", 2560),
];
