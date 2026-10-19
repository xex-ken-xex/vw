//! ファイルとフォルダの並び順。フォルダツリーの表示と、前後の画像への移動に使う。

use std::cmp::Ordering;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// 並べる基準。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortKey {
    /// ファイル名。数字の並びは数値として比べる（img2 → img10）。
    #[default]
    NameNatural,
    /// ファイル名を 1 文字ずつ比べる（img10 → img2）。
    NameString,
    /// 更新日時。
    Modified,
}

impl SortKey {
    pub const ALL: [SortKey; 3] = [SortKey::NameNatural, SortKey::NameString, SortKey::Modified];

    pub fn title(self) -> &'static str {
        match self {
            SortKey::NameNatural => "Name (numbers by value)",
            SortKey::NameString => "Name (character by character)",
            SortKey::Modified => "Modified date",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SortOrder {
    pub key: SortKey,
    pub descending: bool,
}

/// 並べる項目の、順序を決めるのに使う情報。
pub struct SortItem<'a> {
    pub name: &'a str,
    pub is_dir: bool,
    /// [`SortKey::Modified`] のときだけ使う。取れなかった項目は最も古いものとして扱う。
    pub modified: Option<SystemTime>,
}

/// フォルダを常にファイルより前に置き、その中を `order` の順に並べる。
/// 降順にしてもフォルダが前にあるのは変えない。フォルダを辿る操作の位置が
/// 並び順で上下に飛ぶと迷うため。
pub fn compare(order: SortOrder, a: &SortItem, b: &SortItem) -> Ordering {
    let by_key = match order.key {
        SortKey::NameNatural => natural_cmp(a.name, b.name),
        SortKey::NameString => string_cmp(a.name, b.name),
        // 同じ日時のもの（コピーでまとめて作られたファイルなど）は名前の順にして、
        // 開くたびに順番が変わらないようにする。
        SortKey::Modified => a
            .modified
            .cmp(&b.modified)
            .then_with(|| natural_cmp(a.name, b.name)),
    };
    let by_key = if order.descending {
        by_key.reverse()
    } else {
        by_key
    };
    b.is_dir.cmp(&a.is_dir).then(by_key)
}

/// 大文字小文字を無視して 1 文字ずつ比べる。無視して同じなら元の文字列で決める。
pub fn string_cmp(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// 数字の並びを数値として比べる。数字以外は大文字小文字を無視して 1 文字ずつ比べる。
/// 値が同じ数字（`7` と `007`）は短い方を前にし、それでも同じなら元の文字列で決める。
///
/// 数字の並びを整数に変換して比べないのは、桁数の多い連番（日時を並べた名前など）が
/// u64 にも収まらないことがあるため。先頭の 0 を除いた桁数と文字で比べれば桁数に制限がない。
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.len().cmp(&b.len()).then_with(|| a.cmp(b)),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let n = take_digits(&mut x);
                let m = take_digits(&mut y);
                let (n_trim, m_trim) = (n.trim_start_matches('0'), m.trim_start_matches('0'));
                let ord = n_trim
                    .len()
                    .cmp(&m_trim.len())
                    .then_with(|| n_trim.cmp(m_trim));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(c), Some(d)) => {
                let ord = c.to_lowercase().cmp(d.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                x.next();
                y.next();
            }
        }
    }
}

fn take_digits(it: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut s = String::new();
    while let Some(c) = it.next_if(char::is_ascii_digit) {
        s.push(c);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn sorted(mut names: Vec<&str>, cmp: fn(&str, &str) -> Ordering) -> Vec<&str> {
        names.sort_by(|a, b| cmp(a, b));
        names
    }

    #[test]
    fn natural_order_compares_numbers_by_value() {
        let names = vec!["img10.png", "img2.png", "img1.png"];
        assert_eq!(
            sorted(names, natural_cmp),
            ["img1.png", "img2.png", "img10.png"]
        );
    }

    #[test]
    fn string_order_compares_character_by_character() {
        let names = vec!["img10.png", "img2.png", "img1.png"];
        assert_eq!(
            sorted(names, string_cmp),
            ["img1.png", "img10.png", "img2.png"]
        );
    }

    #[test]
    fn both_name_orders_ignore_case() {
        assert_eq!(sorted(vec!["b", "A", "c"], natural_cmp), ["A", "b", "c"]);
        assert_eq!(sorted(vec!["b", "A", "c"], string_cmp), ["A", "b", "c"]);
    }

    #[test]
    fn natural_order_handles_numbers_longer_than_u64() {
        let big = "x123456789012345678901234567890";
        let bigger = "x223456789012345678901234567890";
        assert_eq!(natural_cmp(big, bigger), Ordering::Less);
        assert_eq!(natural_cmp("x9", big), Ordering::Less);
    }

    #[test]
    fn leading_zeros_do_not_change_the_value() {
        assert_eq!(
            sorted(vec!["p010", "p9", "p0011"], natural_cmp),
            ["p9", "p010", "p0011"]
        );
        assert_eq!(natural_cmp("p7", "p007"), Ordering::Less);
    }

    #[test]
    fn shorter_name_with_same_prefix_comes_first() {
        assert_eq!(natural_cmp("img", "img1"), Ordering::Less);
        assert_eq!(natural_cmp("a.png", "a.png.bak"), Ordering::Less);
    }

    fn item(name: &str, is_dir: bool, secs: u64) -> SortItem<'_> {
        SortItem {
            name,
            is_dir,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs)),
        }
    }

    fn order_of(order: SortOrder, items: &mut [SortItem]) -> Vec<String> {
        items.sort_by(|a, b| compare(order, a, b));
        items.iter().map(|i| i.name.to_owned()).collect()
    }

    #[test]
    fn modified_order_puts_older_first_and_breaks_ties_by_name() {
        let order = SortOrder {
            key: SortKey::Modified,
            descending: false,
        };
        let mut items = [
            item("new", false, 30),
            item("b", false, 10),
            item("a", false, 10),
        ];
        assert_eq!(order_of(order, &mut items), ["a", "b", "new"]);
    }

    #[test]
    fn unknown_modified_date_counts_as_oldest() {
        let order = SortOrder {
            key: SortKey::Modified,
            descending: false,
        };
        let unknown = SortItem {
            name: "z",
            is_dir: false,
            modified: None,
        };
        let mut items = [item("a", false, 10), unknown];
        assert_eq!(order_of(order, &mut items), ["z", "a"]);
    }

    #[test]
    fn descending_reverses_the_order_but_keeps_folders_first() {
        let order = SortOrder {
            key: SortKey::NameNatural,
            descending: true,
        };
        let mut items = [
            item("f1", false, 0),
            item("d1", true, 0),
            item("f2", false, 0),
            item("d2", true, 0),
        ];
        assert_eq!(order_of(order, &mut items), ["d2", "d1", "f2", "f1"]);
    }
}
