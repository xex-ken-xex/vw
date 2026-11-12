//! FITS のヘッダ（80 文字のカードの並び）と、HDU の並び。
//!
//! カードは fitsrs の解釈を通さず、ファイルの文字のまま読む。fitsrs のカードは値を型ごとに
//! 解釈し直したもので、`CONTINUE` で続く長い文字列や `HIERARCH` の名前、`COMMENT` の並びを
//! メタデータ欄にファイルどおりの順で出すのに向かないため。

use vw_core::DecodeError;

/// FITS のブロックの大きさ。ヘッダもデータもこの倍数で並ぶ。
pub const BLOCK: usize = 2880;
const CARD: usize = 80;

/// 読む HDU の上限。壊れたファイルで延々と読まないように。
const MAX_HDUS: usize = 1000;

/// カード 1 枚。値は引用符を外し、`CONTINUE` で続く文字列はつないである。
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub key: String,
    /// `COMMENT` や `HISTORY` のような値のないカードは、本文を値にする。
    pub value: String,
    /// 値が引用符で囲まれた文字列だったか。
    pub quoted: bool,
}

/// HDU 1 つ。データ部の位置はファイルの先頭からのバイト数。
#[derive(Debug, Clone)]
pub struct Hdu {
    pub cards: Vec<Card>,
    pub data_start: usize,
    pub data_len: usize,
    /// 規格に外れたカード。`(どのカードか, 何が外れたか)`。読めた分は `cards` に入れてある。
    pub problems: Vec<(String, String)>,
    /// `-TAB` の軸の、表から引いた値。`(軸の番号（1 から）, 表)`。
    pub lookups: Vec<(usize, Lookup)>,
}

/// `-TAB` の軸の表（WCS の論文 III の 6 章）。どちらも表に書いたままの値。
#[derive(Debug, Clone, PartialEq)]
pub struct Lookup {
    /// 番号の列（`PSi_2`）。なければ番号をそのまま使う。
    pub index: Option<Vec<f64>>,
    /// 座標の列（`PSi_1`）の値。
    pub coords: Vec<f64>,
}

impl Hdu {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.cards
            .iter()
            .find(|c| c.key == key)
            .map(|c| c.value.as_str())
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        let v = self.get(key)?.trim();
        v.parse().ok().or_else(|| {
            // 整数のカードを 32768.0 のように実数で書くソフトがある。
            let f: f64 = v.replace(['D', 'd'], "E").parse().ok()?;
            (f.fract() == 0.0 && f.abs() < 9.0e18).then_some(f as i64)
        })
    }

    pub fn float(&self, key: &str) -> Option<f64> {
        // Fortran 流の指数（1.0D+03）も読む。
        self.get(key)?.trim().replace(['D', 'd'], "E").parse().ok()
    }

    pub fn logical(&self, key: &str) -> bool {
        self.get(key).is_some_and(|v| v.trim() == "T")
    }

    /// `XTENSION` の値。Primary HDU なら `None`。
    pub fn xtension(&self) -> Option<&str> {
        self.get("XTENSION").map(str::trim)
    }

    /// `NAXIS1`〜`NAXISn`（`prefix` が `"Z"` なら圧縮前の `ZNAXISn`）。
    pub fn axes(&self, prefix: &str) -> Option<Vec<usize>> {
        let n = self.int(&format!("{prefix}NAXIS"))?;
        if !(0..=999).contains(&n) {
            return None;
        }
        (1..=n)
            .map(|i| {
                self.int(&format!("{prefix}NAXIS{i}"))
                    .and_then(|v| usize::try_from(v).ok())
            })
            .collect()
    }
}

/// 先頭が FITS の印（`SIMPLE  =` で値が T）か。
pub fn is_fits(header: &[u8]) -> bool {
    header.len() >= 30 && header.starts_with(b"SIMPLE  =") && header[29] == b'T'
}

/// ファイルの HDU を順に読む。2 つ目以降が壊れていても、読めたところまでを返す。
pub fn hdus(data: &[u8]) -> Result<Vec<Hdu>, DecodeError> {
    if !is_fits(data) {
        return Err(DecodeError::Invalid("not a FITS file".into()));
    }
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() && out.len() < MAX_HDUS {
        match hdu_at(data, at) {
            Ok(hdu) => {
                at = hdu
                    .data_start
                    .saturating_add(hdu.data_len.next_multiple_of(BLOCK));
                out.push(hdu);
            }
            Err(e) if out.is_empty() => return Err(e),
            // 後ろに付いた余分なバイトや壊れた拡張は、最初の画像を見る邪魔にしない。
            Err(e) => {
                let number = out.len();
                out[0].problems.push((
                    format!("HDU {number} (offset {at})"),
                    format!("cannot be read, so it and the HDUs after it are skipped: {e}"),
                ));
                break;
            }
        }
    }
    crate::table::resolve_lookups(data, &mut out);
    Ok(out)
}

pub(crate) fn hdu_at(data: &[u8], start: usize) -> Result<Hdu, DecodeError> {
    let mut lines: Vec<&[u8]> = Vec::new();
    let mut at = start;
    loop {
        let line = data
            .get(at..at + CARD)
            .ok_or_else(|| DecodeError::Invalid("FITS header has no END".into()))?;
        at += CARD;
        if line.starts_with(b"END") && line[3..].iter().all(|&b| b == b' ') {
            break;
        }
        lines.push(line);
    }
    let (cards, problems) = parse_cards(&lines, start);
    let data_start = (at - start).next_multiple_of(BLOCK) + start;
    let mut hdu = Hdu {
        cards,
        data_start,
        data_len: 0,
        problems,
        lookups: Vec::new(),
    };
    hdu.data_len = data_len(&hdu)
        .ok_or_else(|| DecodeError::Invalid("FITS header has broken NAXIS or BITPIX".into()))?;
    Ok(hdu)
}

/// データ部のバイト数（規格 4.0 の式 2）。`|BITPIX| / 8 × GCOUNT × (PCOUNT + NAXIS1 × … × NAXISn)`。
fn data_len(hdu: &Hdu) -> Option<usize> {
    let bitpix = hdu.int("BITPIX")?;
    if ![8, 16, 32, 64, -32, -64].contains(&bitpix) {
        return None;
    }
    let axes = hdu.axes("")?;
    if axes.is_empty() {
        return Some(0);
    }
    let count = axes.iter().try_fold(1usize, |n, &a| n.checked_mul(a))?;
    let pcount = usize::try_from(hdu.int("PCOUNT").unwrap_or(0)).ok()?;
    let gcount = usize::try_from(hdu.int("GCOUNT").unwrap_or(1)).ok()?;
    (bitpix.unsigned_abs() as usize / 8)
        .checked_mul(gcount)?
        .checked_mul(count.checked_add(pcount)?)
}

/// `start` はヘッダの先頭の、ファイルの中の位置。規格に外れたカードの位置を書くのに使う。
fn parse_cards(lines: &[&[u8]], start: usize) -> (Vec<Card>, Vec<(String, String)>) {
    let mut cards: Vec<Card> = Vec::new();
    let mut problems = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        // ヘッダは ASCII の決まり。外れた文字を 1 文字に置き換え、位置で切り出せるようにする。
        let text: String = line
            .iter()
            .map(|&b| if b.is_ascii() { b as char } else { '?' })
            .collect();
        let key = text[..8.min(text.len())].trim_end();
        let place = || format!("card {} {key} (offset {})", i + 1, start + i * CARD);
        let outside = line.iter().filter(|b| !b.is_ascii()).count();
        if outside > 0 {
            problems.push((
                place(),
                format!(
                    "{outside} byte(s) outside ASCII, shown as '?': {}",
                    vw_core::image::problem_value(text.trim_end())
                ),
            ));
        }
        if !key
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        {
            problems.push((
                place(),
                "keyword has characters other than A-Z, 0-9, '-' and '_'".to_owned(),
            ));
        }
        if key == "CONTINUE" {
            // 長い文字列の続き。前の文字列の末尾の & を外してつなぐ。
            if let Some(prev) = cards.last_mut().filter(|c| c.quoted)
                && let Some(base) = prev.value.strip_suffix('&')
            {
                let more = split_value(&text[8..]);
                if more.quote == Quote::Unclosed {
                    problems.push((place(), "string has no closing quote".to_owned()));
                }
                let more = more.text;
                prev.value = format!("{base}{more}");
                continue;
            }
        }
        if key == "HIERARCH" {
            // ESO の長い名前。`HIERARCH ESO DET DIT = 1.0 / comment`
            if let Some((name, rest)) = text[8..].split_once('=') {
                let value = split_value(rest);
                if value.quote == Quote::Unclosed {
                    problems.push((place(), "string has no closing quote".to_owned()));
                }
                cards.push(Card {
                    key: format!("HIERARCH {}", name.trim()),
                    quoted: value.quote != Quote::None,
                    value: value.text,
                });
                continue;
            }
        }
        if key.is_empty() && text.trim().is_empty() {
            continue;
        }
        if text.get(8..10) == Some("= ") {
            let value = split_value(&text[10..]);
            if value.quote == Quote::Unclosed {
                problems.push((place(), "string has no closing quote".to_owned()));
            }
            cards.push(Card {
                key: key.to_owned(),
                quoted: value.quote != Quote::None,
                value: value.text,
            });
        } else {
            // COMMENT、HISTORY、空の名前の注記。本文をそのまま値にする。
            cards.push(Card {
                key: key.to_owned(),
                value: text.get(8..).unwrap_or_default().trim_end().to_owned(),
                quoted: false,
            });
        }
    }
    (cards, problems)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quote {
    /// 文字列でない値（数や T/F）。
    None,
    Closed,
    /// 閉じる引用符がないまま行が終わった。
    Unclosed,
}

struct Value {
    text: String,
    quote: Quote,
}

/// `= ` の後ろを、値とコメントに分ける。文字列は引用符を外し、`''` を `'` に戻す。
fn split_value(s: &str) -> Value {
    let s = s.trim_start();
    if let Some(body) = s.strip_prefix('\'') {
        let mut out = String::new();
        let mut quote = Quote::Unclosed;
        let mut chars = body.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\'' {
                if chars.peek() == Some(&'\'') {
                    chars.next();
                    out.push('\'');
                } else {
                    quote = Quote::Closed;
                    break;
                }
            } else {
                out.push(c);
            }
        }
        // 文字列の後ろの空白は意味を持たない（規格 4.2.1.1）。
        return Value {
            text: out.trim_end().to_owned(),
            quote,
        };
    }
    Value {
        text: s.split('/').next().unwrap_or_default().trim().to_owned(),
        quote: Quote::None,
    }
}
