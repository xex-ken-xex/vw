//! 項目ごとに読める TOML の読み取り。
//!
//! serde でファイル全体を構造体に読み込むと、1 項目の型が違うだけで全体が読めなくなる。
//! 壊れた項目だけを既定値にして残りは生かすため、項目を 1 つずつ取り出して変換する。

use serde::de::DeserializeOwned;

/// TOML の表から `[節] 項目` を 1 つずつ読む。読めなかった項目の説明を溜める。
pub struct Reader<'a> {
    table: &'a toml::Table,
    pub problems: Vec<String>,
    /// 表になっていなかった節。同じ節について何度も報告しないために覚えておく。
    bad_sections: Vec<String>,
}

impl<'a> Reader<'a> {
    pub fn new(table: &'a toml::Table) -> Self {
        Self {
            table,
            problems: Vec::new(),
            bad_sections: Vec::new(),
        }
    }

    /// `[name]` の表。`fits.calibration` のように `.` で区切れば入れ子の表。
    /// ないときと、表でないとき（報告する）は `None`。
    pub fn section(&mut self, name: &str) -> Option<&'a toml::Table> {
        let (outer, inner) = match name.rsplit_once('.') {
            Some((outer, inner)) => (self.section(outer)?, inner),
            None => (self.table, name),
        };
        match outer.get(inner)? {
            toml::Value::Table(t) => Some(t),
            other => {
                if !self.bad_sections.iter().any(|s| s == name) {
                    self.bad_sections.push(name.to_owned());
                    self.problems.push(format!(
                        "[{name}]: expected a section, found {} (using defaults)",
                        other.type_str()
                    ));
                }
                None
            }
        }
    }

    /// `[section] key` を `T` として読む。ないときは黙って `None`、読めないときは報告して `None`。
    pub fn get<T: DeserializeOwned>(&mut self, section: &str, key: &str) -> Option<T> {
        self.get_valid(section, key, |_| true, "")
    }

    /// [`Self::get`] に加えて、`valid` を満たさない値も読めなかったものとして扱う。
    /// `rule` は報告に添える、満たすべき条件の説明。
    pub fn get_valid<T: DeserializeOwned>(
        &mut self,
        section: &str,
        key: &str,
        valid: impl Fn(&T) -> bool,
        rule: &str,
    ) -> Option<T> {
        let value = self.section(section)?.get(key)?;
        match value.clone().try_into::<T>() {
            Ok(v) if valid(&v) => Some(v),
            Ok(_) => {
                self.problems.push(format!(
                    "{section}.{key}: {value} is out of range, {rule} (using default)"
                ));
                None
            }
            Err(_) => {
                self.problems.push(format!(
                    "{section}.{key}: invalid value {value} (using default)"
                ));
                None
            }
        }
    }
}

impl<'a> Reader<'a> {
    /// 1 つの節の項目を続けて読むときの、節の名前を省いた入口。
    pub fn at<'r>(&'r mut self, section: &'static str) -> Section<'r, 'a> {
        Section {
            reader: self,
            section,
        }
    }
}

/// [`Reader`] の 1 つの節。
pub struct Section<'r, 'a> {
    reader: &'r mut Reader<'a>,
    section: &'static str,
}

impl<'a> Section<'_, 'a> {
    /// 入れ子の節を読むための、元の読み取り。
    pub fn reader(&mut self) -> &mut Reader<'a> {
        self.reader
    }

    pub fn get<T: DeserializeOwned>(&mut self, key: &str) -> Option<T> {
        self.reader.get(self.section, key)
    }

    pub fn get_valid<T: DeserializeOwned>(
        &mut self,
        key: &str,
        valid: impl Fn(&T) -> bool,
        rule: &str,
    ) -> Option<T> {
        self.reader.get_valid(self.section, key, valid, rule)
    }

    /// 項目の中の一部だけを捨てたときなど、読み取りとは別に報告する。
    pub fn problem(&mut self, key: &str, text: impl std::fmt::Display) {
        self.reader
            .problems
            .push(format!("{}.{key}: {text}", self.section));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> toml::Table {
        text.parse().unwrap()
    }

    #[test]
    fn reads_a_value_of_the_right_type() {
        let t = table("[a]\nx = 3");
        let mut r = Reader::new(&t);
        assert_eq!(r.get::<i64>("a", "x"), Some(3));
        assert!(r.problems.is_empty());
    }

    #[test]
    fn nested_sections_are_read_with_dotted_names() {
        let t = table("[a.b]\nx = 3");
        let mut r = Reader::new(&t);
        assert_eq!(r.get::<i64>("a.b", "x"), Some(3));
        assert_eq!(r.get::<i64>("a", "x"), None);
        assert!(r.problems.is_empty());
    }

    #[test]
    fn missing_values_and_sections_are_silent() {
        let t = table("[a]\nx = 3");
        let mut r = Reader::new(&t);
        assert_eq!(r.get::<i64>("a", "y"), None);
        assert_eq!(r.get::<i64>("b", "x"), None);
        assert!(r.problems.is_empty());
    }

    #[test]
    fn wrong_type_is_reported() {
        let t = table("[a]\nx = \"three\"");
        let mut r = Reader::new(&t);
        assert_eq!(r.get::<i64>("a", "x"), None);
        assert_eq!(r.problems.len(), 1);
        assert!(r.problems[0].starts_with("a.x:"), "{}", r.problems[0]);
    }

    #[test]
    fn value_failing_validation_is_reported_with_the_rule() {
        let t = table("[a]\nx = 30");
        let mut r = Reader::new(&t);
        assert_eq!(
            r.get_valid::<i64>("a", "x", |v| *v < 10, "must be below 10"),
            None
        );
        assert!(
            r.problems[0].contains("must be below 10"),
            "{}",
            r.problems[0]
        );
    }

    #[test]
    fn non_table_section_is_reported_once() {
        let t = table("a = 1");
        let mut r = Reader::new(&t);
        assert_eq!(r.get::<i64>("a", "x"), None);
        assert_eq!(r.get::<i64>("a", "y"), None);
        assert_eq!(r.problems.len(), 1);
    }
}
