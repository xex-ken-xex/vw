//! 行と列の表。画像ではなく表として描くデータ（FITS の BINTABLE・TABLE など）。
//!
//! 行は描くときに必要な分だけ読む（[`Table::cell`]）。数百万行の表でも、先に全部を文字にすると
//! 時間もメモリもかかるため。

use std::fmt::Debug;

/// 列の見出し。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Column {
    pub name: String,
    /// 値の単位。分からなければ空。
    pub unit: String,
    /// 型の書き方（FITS の `TFORM` など）。見出しの補足に出す。
    pub format: String,
}

/// 表。形式ごとのデコーダが作る。
pub trait Table: Debug + Send + Sync {
    fn columns(&self) -> &[Column];

    fn rows(&self) -> usize;

    /// `row` 行目（0 から）の `column` 列目のセルを、画面に出す文字にする。読めないセルは空。
    fn cell(&self, row: usize, column: usize) -> String;

    /// 表の大きさの説明（`5 columns x 4,000 rows`）。形式によっては種類も付ける。
    fn summary(&self) -> String {
        size_text(self.columns().len(), self.rows())
    }
}

/// 列と行の数（`1 column x 1 row`、`11 columns x 100,000 rows`）。
pub fn size_text(columns: usize, rows: usize) -> String {
    let plural = |n: usize, one: &str, many: &str| {
        format!("{} {}", group_digits(n), if n == 1 { one } else { many })
    };
    format!(
        "{} x {}",
        plural(columns, "column", "columns"),
        plural(rows, "row", "rows")
    )
}

/// 3 桁ごとにカンマを入れる（`1234567` → `1,234,567`）。
pub fn group_digits(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `rows` の行を、見出しの行を先頭にしたタブ区切りの文字にする（表計算に貼れる形）。
pub fn tab_separated(table: &dyn Table, rows: impl IntoIterator<Item = usize>) -> String {
    let columns = table.columns().len();
    let mut out: String = table
        .columns()
        .iter()
        .map(|c| c.name.as_str())
        .collect::<Vec<_>>()
        .join("\t");
    for row in rows {
        out.push('\n');
        let cells: Vec<String> = (0..columns)
            .map(|c| clean_cell(&table.cell(row, c)))
            .collect();
        out.push_str(&cells.join("\t"));
    }
    out
}

/// タブと改行は、貼った先で列や行がずれるので空白にする。
fn clean_cell(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Two(Vec<Column>);

    impl Table for Two {
        fn columns(&self) -> &[Column] {
            &self.0
        }
        fn rows(&self) -> usize {
            3
        }
        fn cell(&self, row: usize, column: usize) -> String {
            if column == 1 && row == 2 {
                "a\tb".into()
            } else {
                format!("{row}{column}")
            }
        }
    }

    fn two() -> Two {
        Two(["WAVE", "FLUX"]
            .map(|n| Column {
                name: n.into(),
                ..Column::default()
            })
            .to_vec())
    }

    #[test]
    fn copied_rows_start_with_the_column_names_and_are_tab_separated() {
        assert_eq!(tab_separated(&two(), [0, 2]), "WAVE\tFLUX\n00\t01\n20\ta b");
    }

    #[test]
    fn row_counts_are_written_with_thousands_separators() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1000), "1,000");
        assert_eq!(group_digits(1234567), "1,234,567");
        assert_eq!(two().summary(), "2 columns x 3 rows");
        assert_eq!(size_text(1, 1), "1 column x 1 row");
    }
}
