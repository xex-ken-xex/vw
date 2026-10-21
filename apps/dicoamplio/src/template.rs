//! printf 風のテンプレート。`%(名前)06d` のように変数名を括弧で指定する。
//!
//! 変換: `d`(整数) `s`(文字列) `f`(実数) `x` `X`(16進)、`%%` は `%` 一つ。
//! 書式: `-`(左寄せ) `0`(ゼロ埋め) `+`(符号) 幅 `.精度`。
//! 例: `EVAL%(p)06d` → EVAL000012、`%(weight).1f` → 62.5
//!
//! 日時の変数(`now_` で始まる名前)は、名前の後ろでずらせる。`+1d*s` は 1 日 × s、`-90m` は 90 分前、
//! `~30m` は ± 30 分の中のランダム。単位は `d` `h` `m` `s`。例: `%(now_date+1d*s)s`、`%(now_time~30m)s`

use anyhow::{bail, Context, Result};
use std::collections::HashMap;

#[derive(Debug, Clone)]
enum Piece {
    Lit(String),
    Spec {
        name: String,
        shift: Option<Shift>,
        left: bool,
        zero: bool,
        plus: bool,
        width: usize,
        prec: Option<usize>,
        conv: char,
    },
}

/// 日時の変数のずらし。`now_date+1d*s~30m` の `+1d*s~30m`。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shift {
    /// 決まったずらし(秒)と、それに掛ける整数の変数
    pub steps: Vec<(i64, Option<String>)>,
    /// ランダムのずらしの幅(秒)。-幅〜+幅 の中から選ぶ
    pub jitter: i64,
}

/// ずらせる変数の名前の頭。ほかの名前は `-` などを含んでも、そのまま 1 つの名前(CSV の列名など)として読む
const SHIFTABLE_PREFIX: &str = "now_";

#[derive(Debug, Clone)]
pub struct Template {
    pieces: Vec<Piece>,
}

/// 日時の変数の名前、決まったずらし(秒)、ランダムの幅(秒)から値を出す。
pub type Clock<'a> = dyn Fn(&str, i64, i64) -> Result<String> + 'a;

/// 実行時の値。整数の変数、文字列の変数(元ファイルの値など)、CSV の列。
/// 名前が重なったときは、整数 → 文字列の変数 → CSV の順に見る。
pub struct Vars<'a> {
    pub ints: &'a [(&'static str, i64)],
    pub strs: &'a [(&'static str, &'a str)],
    pub csv: Option<&'a HashMap<String, String>>,
    /// 日時の変数をずらした値。None ならずらしは使えない
    pub clock: Option<&'a Clock<'a>>,
}

impl Template {
    pub fn parse(src: &str) -> Result<Template> {
        let cs: Vec<char> = src.chars().collect();
        let mut pieces = Vec::new();
        let mut lit = String::new();
        let mut k = 0;
        while k < cs.len() {
            if cs[k] != '%' {
                lit.push(cs[k]);
                k += 1;
                continue;
            }
            k += 1;
            if k < cs.len() && cs[k] == '%' {
                lit.push('%');
                k += 1;
                continue;
            }
            if k >= cs.len() || cs[k] != '(' {
                bail!("`%` の次は `(名前)` か `%` が必要: {src}");
            }
            let close = cs[k..]
                .iter()
                .position(|&c| c == ')')
                .context("`)` がない")?
                + k;
            let raw: String = cs[k + 1..close].iter().collect();
            let (name, shift) = split_shift(&raw).with_context(|| format!("`%({raw})`"))?;
            k = close + 1;
            let (mut left, mut zero, mut plus) = (false, false, false);
            while k < cs.len() && matches!(cs[k], '-' | '0' | '+') {
                match cs[k] {
                    '-' => left = true,
                    '0' => zero = true,
                    _ => plus = true,
                }
                k += 1;
            }
            let mut width = 0usize;
            while k < cs.len() && cs[k].is_ascii_digit() {
                width = width * 10 + cs[k].to_digit(10).unwrap() as usize;
                k += 1;
            }
            let mut prec = None;
            if k < cs.len() && cs[k] == '.' {
                k += 1;
                let mut n = 0usize;
                while k < cs.len() && cs[k].is_ascii_digit() {
                    n = n * 10 + cs[k].to_digit(10).unwrap() as usize;
                    k += 1;
                }
                prec = Some(n);
            }
            let conv = *cs.get(k).context("変換文字(d s f x X)がない")?;
            if !matches!(conv, 'd' | 's' | 'f' | 'x' | 'X') {
                bail!("未対応の変換 `%{conv}`: {src}");
            }
            k += 1;
            if !lit.is_empty() {
                pieces.push(Piece::Lit(std::mem::take(&mut lit)));
            }
            pieces.push(Piece::Spec {
                name,
                shift,
                left,
                zero,
                plus,
                width,
                prec,
                conv,
            });
        }
        if !lit.is_empty() {
            pieces.push(Piece::Lit(lit));
        }
        Ok(Template { pieces })
    }

    /// CSV の列を使っているか(整数の変数だけなら、CSV の行を変えても値は変わらない)。
    pub fn uses_csv(&self, columns: &[String]) -> bool {
        self.names().any(|n| columns.iter().any(|c| c == n))
    }

    /// 使っている変数名(起動時の検査用)。
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.pieces.iter().filter_map(|p| match p {
            Piece::Spec { name, .. } => Some(name.as_str()),
            _ => None,
        })
    }

    /// ずらしの付いた変数と、そのずらし(起動時の検査用)。
    pub fn shifts(&self) -> impl Iterator<Item = (&str, &Shift)> {
        self.pieces.iter().filter_map(|p| match p {
            Piece::Spec {
                name,
                shift: Some(s),
                ..
            } => Some((name.as_str(), s)),
            _ => None,
        })
    }

    pub fn render(&self, v: &Vars) -> Result<String> {
        let mut out = String::new();
        for p in &self.pieces {
            match p {
                Piece::Lit(s) => out.push_str(s),
                Piece::Spec {
                    name,
                    shift,
                    left,
                    zero,
                    plus,
                    width,
                    prec,
                    conv,
                } => {
                    let shifted = match shift {
                        Some(sh) => Some(shifted(name, sh, v)?),
                        None => None,
                    };
                    let int = match shifted {
                        Some(_) => None,
                        None => v.ints.iter().find(|(n, _)| n == name).map(|(_, x)| *x),
                    };
                    let text: Option<&str> = shifted.as_deref().or_else(|| {
                        v.strs
                            .iter()
                            .find(|(n, _)| n == name)
                            .map(|(_, t)| *t)
                            .or_else(|| v.csv.and_then(|m| m.get(name)).map(String::as_str))
                    });
                    let body = match conv {
                        'd' | 'x' | 'X' => {
                            let n = match (int, text) {
                                (Some(n), _) => n,
                                (None, Some(t)) => {
                                    t.trim()
                                        .parse::<f64>()
                                        .with_context(|| format!("`{name}` が数でない: {t}"))?
                                        .round() as i64
                                }
                                _ => bail!("変数 `{name}` がない"),
                            };
                            let digits = match conv {
                                'x' => format!("{:x}", n.unsigned_abs()),
                                'X' => format!("{:X}", n.unsigned_abs()),
                                _ => n.unsigned_abs().to_string(),
                            };
                            let sign = if n < 0 {
                                "-"
                            } else if *plus {
                                "+"
                            } else {
                                ""
                            };
                            pad_num(sign, &digits, *width, *left, *zero)
                        }
                        'f' => {
                            let x = match (int, text) {
                                (Some(n), _) => n as f64,
                                (None, Some(t)) => t
                                    .trim()
                                    .parse::<f64>()
                                    .with_context(|| format!("`{name}` が数でない: {t}"))?,
                                _ => bail!("変数 `{name}` がない"),
                            };
                            let digits = format!("{:.*}", prec.unwrap_or(6), x.abs());
                            let sign = if x < 0.0 {
                                "-"
                            } else if *plus {
                                "+"
                            } else {
                                ""
                            };
                            pad_num(sign, &digits, *width, *left, *zero)
                        }
                        _ => {
                            let mut s = match (int, text) {
                                (_, Some(t)) => t.to_string(),
                                (Some(n), None) => n.to_string(),
                                _ => bail!("変数 `{name}` がない"),
                            };
                            if let Some(p) = prec {
                                s = s.chars().take(*p).collect();
                            }
                            pad(&s, *width, *left)
                        }
                    };
                    out.push_str(&body);
                }
            }
        }
        Ok(out)
    }
}

const SHIFT_FORM: &str = "ずらしは `+1d` `-5m*i` `~30m` の形(単位は d h m s)";

/// `now_date+1d*s` を名前 `now_date` とずらし `+1d*s` に分ける。
fn split_shift(raw: &str) -> Result<(String, Option<Shift>)> {
    let cut = match raw.find(['+', '-', '~']) {
        Some(c) if raw.starts_with(SHIFTABLE_PREFIX) => c,
        _ => return Ok((raw.to_string(), None)),
    };
    let rest: Vec<char> = raw[cut..].chars().filter(|c| !c.is_whitespace()).collect();
    let mut shift = Shift::default();
    let mut has_jitter = false;
    let mut k = 0;
    while k < rest.len() {
        let op = rest[k];
        k += 1;
        let secs = amount(&rest, &mut k)?;
        match op {
            '~' => {
                if has_jitter {
                    bail!("ランダムのずらし `~` は 1 つまで");
                }
                if rest.get(k) == Some(&'*') {
                    bail!("ランダムのずらし `~` には変数を掛けられない");
                }
                has_jitter = true;
                shift.jitter = secs;
            }
            '+' | '-' => {
                let by = if rest.get(k) == Some(&'*') {
                    k += 1;
                    let start = k;
                    while k < rest.len() && (rest[k].is_ascii_alphanumeric() || rest[k] == '_') {
                        k += 1;
                    }
                    if k == start {
                        bail!("`*` の次に変数名がない。{SHIFT_FORM}");
                    }
                    Some(rest[start..k].iter().collect())
                } else {
                    None
                };
                shift.steps.push((if op == '-' { -secs } else { secs }, by));
            }
            c => bail!("`{c}` が読めない。{SHIFT_FORM}"),
        }
    }
    Ok((raw[..cut].trim().to_string(), Some(shift)))
}

/// `30m` のような量を秒にする。
fn amount(cs: &[char], k: &mut usize) -> Result<i64> {
    let start = *k;
    let mut n: i64 = 0;
    while *k < cs.len() && cs[*k].is_ascii_digit() {
        n = n
            .checked_mul(10)
            .and_then(|n| n.checked_add(cs[*k].to_digit(10).unwrap() as i64))
            .context("ずらしの数が大きすぎる")?;
        *k += 1;
    }
    if *k == start {
        bail!("ずらしの数がない。{SHIFT_FORM}");
    }
    let unit = match cs.get(*k) {
        Some('d') => 86_400,
        Some('h') => 3_600,
        Some('m') => 60,
        Some('s') => 1,
        _ => bail!("ずらしの単位がない。{SHIFT_FORM}"),
    };
    *k += 1;
    n.checked_mul(unit).context("ずらしの数が大きすぎる")
}

/// ずらした日時の変数の値。決まったずらしは掛ける変数の値から足し合わせ、ランダムの分は `clock` に任せる。
fn shifted(name: &str, sh: &Shift, v: &Vars) -> Result<String> {
    let clock = v
        .clock
        .with_context(|| format!("`{name}` のずらしはここでは使えない"))?;
    let mut secs: i64 = 0;
    for (step, by) in &sh.steps {
        let times = match by {
            None => 1,
            Some(b) => v
                .ints
                .iter()
                .find(|(n, _)| n == b)
                .map(|(_, x)| *x)
                .with_context(|| format!("ずらしに掛ける整数の変数 `{b}` がない"))?,
        };
        secs = step
            .checked_mul(times)
            .and_then(|x| secs.checked_add(x))
            .context("日時のずらしが大きすぎる")?;
    }
    clock(name, secs, sh.jitter)
}

fn pad(s: &str, width: usize, left: bool) -> String {
    let n = s.chars().count();
    if n >= width {
        return s.to_string();
    }
    let fill = " ".repeat(width - n);
    if left {
        format!("{s}{fill}")
    } else {
        format!("{fill}{s}")
    }
}

fn pad_num(sign: &str, digits: &str, width: usize, left: bool, zero: bool) -> String {
    let n = sign.len() + digits.len();
    if n >= width {
        return format!("{sign}{digits}");
    }
    let k = width - n;
    if left {
        format!("{sign}{digits}{}", " ".repeat(k))
    } else if zero {
        format!("{sign}{}{digits}", "0".repeat(k))
    } else {
        format!("{}{sign}{digits}", " ".repeat(k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(csv: Option<&HashMap<String, String>>) -> Vars<'_> {
        Vars {
            ints: &[
                ("p", 12),
                ("s", 0),
                ("r", 1),
                ("i", 3),
                ("p1", 13),
                ("s1", 1),
                ("r1", 2),
                ("i1", 4),
                ("n", 99),
                ("inst", 7),
            ],
            strs: &[("src_patient_id", "ORIG-7")],
            csv,
            clock: None,
        }
    }

    #[test]
    fn printf() {
        let t = Template::parse("EVAL%(p)06d-%(s1)d %(n)x %%").unwrap();
        assert_eq!(t.render(&vars(None)).unwrap(), "EVAL000012-1 63 %");
    }

    #[test]
    fn csv() {
        let m: HashMap<String, String> = [("name", "Yamada^Taro"), ("weight", "62.46")]
            .map(|(a, b)| (a.into(), b.into()))
            .into();
        let t = Template::parse("%(name)s|%(weight).1f|%(weight)8.2f|%(name)-12s|").unwrap();
        assert_eq!(
            t.render(&vars(Some(&m))).unwrap(),
            "Yamada^Taro|62.5|   62.46|Yamada^Taro |"
        );
    }

    #[test]
    fn string_variables_render_like_csv_columns() {
        let t = Template::parse("%(src_patient_id)s-%(p1)d|%(src_patient_id).4s|").unwrap();
        assert_eq!(t.render(&vars(None)).unwrap(), "ORIG-7-13|ORIG|");
    }

    /// ずらしをそのまま書き出す時計(テンプレートが渡す値を確かめる)。
    fn echo_clock(name: &str, secs: i64, jitter: i64) -> Result<String> {
        Ok(format!("{name}|{secs}|{jitter}"))
    }

    fn render_with_clock(
        src: &str,
        clock: &dyn Fn(&str, i64, i64) -> Result<String>,
    ) -> Result<String> {
        let mut v = vars(None);
        v.clock = Some(clock);
        Template::parse(src)?.render(&v)
    }

    #[test]
    fn shift_is_split_into_steps_with_multipliers_and_a_jitter() {
        let t = Template::parse("%(now_date+1d*s-90m~30m)s").unwrap();
        let shifts: Vec<_> = t.shifts().collect();
        assert_eq!(shifts.len(), 1);
        assert_eq!(shifts[0].0, "now_date");
        assert_eq!(
            *shifts[0].1,
            Shift {
                steps: vec![(86_400, Some("s".into())), (-5_400, None)],
                jitter: 1_800,
            }
        );
        assert_eq!(t.names().collect::<Vec<_>>(), ["now_date"]);
    }

    #[test]
    fn units_are_days_hours_minutes_and_seconds() {
        for (src, secs) in [
            ("+2d", 172_800),
            ("+3h", 10_800),
            ("+4m", 240),
            ("+5s", 5),
            ("-1d", -86_400),
        ] {
            let t = Template::parse(&format!("%(now_time{src})s")).unwrap();
            assert_eq!(t.shifts().next().unwrap().1.steps, [(secs, None)], "{src}");
        }
    }

    #[test]
    fn fixed_steps_are_multiplied_by_loop_counts_and_summed() {
        // i = 3, p = 12: 5 分 × 3 − 1 時間 × 12 + 7 秒
        assert_eq!(
            render_with_clock("%(now_time+5m*i-1h*p+7s~2h)s", &echo_clock).unwrap(),
            "now_time|-42293|7200"
        );
        // 掛ける変数が 0 なら、ずらさない(s = 0)
        assert_eq!(
            render_with_clock("%(now_date+1d*s)s", &echo_clock).unwrap(),
            "now_date|0|0"
        );
        // 1 始まりの番号も掛けられる(s1 = 1)
        assert_eq!(
            render_with_clock("%(now_date-1d*s1)s", &echo_clock).unwrap(),
            "now_date|-86400|0"
        );
    }

    #[test]
    fn spaces_inside_a_shift_are_ignored() {
        assert_eq!(
            render_with_clock("%(now_date + 1d * i ~ 1h)s", &echo_clock).unwrap(),
            "now_date|259200|3600"
        );
    }

    #[test]
    fn shifted_values_take_printf_formats() {
        let clock = |name: &str, _: i64, _: i64| -> Result<String> {
            Ok(match name {
                "now_month" => "3".into(),
                _ => "20261022".into(),
            })
        };
        assert_eq!(
            render_with_clock(
                "%(now_month+1d)02d|%(now_date+1d)d|%(now_date+1d).6s|%(now_date+1d)10s",
                &clock
            )
            .unwrap(),
            "03|20261022|202610|  20261022"
        );
    }

    #[test]
    fn unshifted_variables_next_to_shifted_ones_keep_their_values() {
        let t = "%(now_date+1d)s %(p)d %(src_patient_id)s";
        assert_eq!(
            render_with_clock(t, &echo_clock).unwrap(),
            "now_date|86400|0 12 ORIG-7"
        );
    }

    #[test]
    fn names_without_the_now_prefix_keep_signs_as_part_of_the_name() {
        let m: HashMap<String, String> = [("blood-type", "A+"), ("a~b", "x")]
            .map(|(a, b)| (a.into(), b.into()))
            .into();
        let t = Template::parse("%(blood-type)s %(a~b)s").unwrap();
        assert_eq!(t.shifts().count(), 0);
        assert_eq!(t.render(&vars(Some(&m))).unwrap(), "A+ x");
    }

    #[test]
    fn shift_needs_a_clock() {
        let err = Template::parse("%(now_date+1d)s")
            .unwrap()
            .render(&vars(None))
            .unwrap_err();
        assert!(format!("{err:#}").contains("ここでは使えない"), "{err:#}");
    }

    #[test]
    fn shift_by_a_missing_variable_is_an_error() {
        let err = render_with_clock("%(now_date+1d*zz)s", &echo_clock).unwrap_err();
        assert!(format!("{err:#}").contains("`zz`"), "{err:#}");
        // 文字列の変数は掛けられない
        assert!(render_with_clock("%(now_date+1d*src_patient_id)s", &echo_clock).is_err());
    }

    #[test]
    fn overflowing_shift_is_an_error() {
        assert!(Template::parse("%(now_date+99999999999999999999d)s").is_err());
        assert!(Template::parse("%(now_date+999999999999999d)s").is_err());
        // 掛け算であふれる(n = 99)
        let huge = format!("%(now_date+{}s*n)s", i64::MAX / 10);
        assert!(render_with_clock(&huge, &echo_clock).is_err());
    }

    #[test]
    fn malformed_shifts_are_errors() {
        for src in [
            "%(now_date+)s",
            "%(now_date+1)s",
            "%(now_date+1y)s",
            "%(now_date+d)s",
            "%(now_date+1d*)s",
            "%(now_date+1d/2)s",
            "%(now_date+1d2)s",
            "%(now_date~1m~2m)s",
            "%(now_date~1m*s)s",
            "%(now_date~)s",
        ] {
            assert!(Template::parse(src).is_err(), "{src}");
        }
    }

    #[test]
    fn errors() {
        assert!(Template::parse("%d").is_err());
        assert!(Template::parse("%(p)q").is_err());
        assert!(Template::parse("%(zz)d")
            .unwrap()
            .render(&vars(None))
            .is_err());
    }
}
