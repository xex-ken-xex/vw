//! 地名の表（GeoNames から作った `vw-geonames.tsv.gz`）。撮影地点に一番近い町の名前を付け、
//! 地名から場所を探す。
//!
//! 表の作り方は `tools/geodata/make_geodata.py`。地名データは GeoNames（https://www.geonames.org/）、
//! CC BY 4.0。

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, RwLock};

use crate::settings::Language;

/// 地名を表に出すときの出典。About と地図の画面に出す。
pub const CREDIT: &str = "Place names: GeoNames (https://www.geonames.org/), licensed under CC BY 4.0. Reduced to the columns vw needs.";
pub const CREDIT_URL: &str = "https://www.geonames.org/";

/// 撮影地点から町を探すときの、一番遠い距離。これより遠ければ（海の上など）町の名前は付けない。
const NEAREST_KM: f64 = 30.0;
const EARTH_KM: f64 = 6371.0;

/// 英語と日本語の名前。日本語の名前がない地名もある。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Name {
    pub en: String,
    pub ja: String,
}

impl Name {
    pub fn get(&self, lang: Language) -> &str {
        match lang {
            Language::Japanese if !self.ja.is_empty() => &self.ja,
            _ => &self.en,
        }
    }

    fn matches(&self, query: &str) -> bool {
        self.en.to_lowercase().contains(query) || self.ja.contains(query)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Town {
    pub latitude: f64,
    pub longitude: f64,
    pub country: String,
    /// 国コード.州コード（`JP.40` など）。
    pub region: String,
    pub population: u64,
    pub name: Name,
}

/// 撮影地点の住所（国・都道府県や州・町）。
#[derive(Debug, Clone, PartialEq)]
pub struct Address<'a> {
    pub country: Option<&'a Name>,
    pub region: Option<&'a Name>,
    pub town: &'a Town,
}

impl Address<'_> {
    /// 日本語は大きい順（日本 北海道 札幌市）、英語は小さい順（Sapporo, Hokkaido, Japan）。
    pub fn text(&self, lang: Language) -> String {
        let mut parts: Vec<&str> = [self.country, self.region]
            .into_iter()
            .flatten()
            .map(|n| n.get(lang))
            .collect();
        parts.push(self.town.name.get(lang));
        match lang {
            Language::Japanese => parts.join(" "),
            Language::English => {
                parts.reverse();
                parts.join(", ")
            }
        }
    }
}

/// 地名で探した結果の 1 つ。
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub label: String,
    pub latitude: f64,
    pub longitude: f64,
    /// 地図をどこまで拡大して見せるか（町・州・国で変える）。
    pub zoom: f64,
}

#[derive(Debug, Default)]
pub struct Gazetteer {
    countries: HashMap<String, Name>,
    regions: HashMap<String, Name>,
    towns: Vec<Town>,
    /// 緯度・経度 1 度ごとの升に入る町の番号。近い町を全部の町から探さないため。
    grid: HashMap<(i32, i32), Vec<usize>>,
}

impl Gazetteer {
    pub fn load(path: &Path) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut text = String::new();
        flate2::read::GzDecoder::new(file)
            .read_to_string(&mut text)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut lines = text.lines();
        if lines.next() != Some("# vw-geonames 1") {
            return Err("not a vw place-name table".into());
        }
        let mut g = Gazetteer::default();
        let name = |en: &str, ja: Option<&&str>| Name {
            en: en.to_owned(),
            ja: ja.copied().unwrap_or_default().to_owned(),
        };
        for line in lines {
            let f: Vec<&str> = line.split('\t').collect();
            match f.as_slice() {
                ["C", code, en, ja @ ..] => {
                    g.countries.insert((*code).to_owned(), name(en, ja.first()));
                }
                ["A", code, en, ja @ ..] => {
                    g.regions.insert((*code).to_owned(), name(en, ja.first()));
                }
                ["P", lat, lon, cc, a1, pop, en, ja @ ..] => {
                    let (Ok(latitude), Ok(longitude)) = (lat.parse(), lon.parse()) else {
                        continue;
                    };
                    g.towns.push(Town {
                        latitude,
                        longitude,
                        country: (*cc).to_owned(),
                        region: format!("{cc}.{a1}"),
                        population: pop.parse().unwrap_or(0),
                        name: name(en, ja.first()),
                    });
                }
                _ => {}
            }
        }
        for (i, t) in g.towns.iter().enumerate() {
            g.grid
                .entry(cell(t.latitude, t.longitude))
                .or_default()
                .push(i);
        }
        Ok(g)
    }

    pub fn town_count(&self) -> usize {
        self.towns.len()
    }

    /// 一番近い町と、その州・国。
    pub fn address(&self, latitude: f64, longitude: f64) -> Option<Address<'_>> {
        let town = self.nearest(latitude, longitude)?;
        Some(Address {
            country: self.countries.get(&town.country),
            region: self.regions.get(&town.region),
            town,
        })
    }

    fn nearest(&self, latitude: f64, longitude: f64) -> Option<&Town> {
        let (cy, cx) = cell(latitude, longitude);
        // 経度 1 度の幅は極に近いほど狭いので、その分だけ東西に広く探す。
        let lon_km = (111.32 * latitude.to_radians().cos()).max(1.0);
        let wide = (NEAREST_KM / lon_km).ceil() as i32;
        let mut best: Option<(f64, &Town)> = None;
        for dy in -1..=1 {
            for dx in -wide..=wide {
                // 経度は -180〜179 の升に回す（日付変更線の向こうの町も探す）。
                let x = (cx + dx + 180).rem_euclid(360) - 180;
                for &i in self.grid.get(&(cy + dy, x)).into_iter().flatten() {
                    let t = &self.towns[i];
                    let d = distance_km(latitude, longitude, t.latitude, t.longitude);
                    if d <= NEAREST_KM && best.is_none_or(|(b, _)| d < b) {
                        best = Some((d, t));
                    }
                }
            }
        }
        best.map(|(_, t)| t)
    }

    /// 名前に `query` を含む国・州・町。国と州が先、町は人口の多い順。
    pub fn search(&self, query: &str, lang: Language, limit: usize) -> Vec<Found> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        // 国と州の位置は、そこで一番大きい町にする（表に国や州の中心は入っていないため）。
        let largest = |keep: &dyn Fn(&Town) -> bool| {
            self.towns
                .iter()
                .filter(|t| keep(t))
                .max_by_key(|t| t.population)
        };
        let mut out = Vec::new();
        let mut countries: Vec<(&String, &Name)> = self
            .countries
            .iter()
            .filter(|(_, n)| n.matches(&q))
            .collect();
        countries.sort();
        for (code, name) in countries {
            if let Some(t) = largest(&|t| &t.country == code) {
                out.push(found(name.get(lang).to_owned(), t, 5.0));
            }
        }
        let mut regions: Vec<(&String, &Name)> =
            self.regions.iter().filter(|(_, n)| n.matches(&q)).collect();
        regions.sort();
        for (code, name) in regions {
            if let Some(t) = largest(&|t| &t.region == code) {
                let country = self.countries.get(&t.country).map(|c| c.get(lang));
                out.push(found(join(lang, name.get(lang), country), t, 8.0));
            }
        }
        let mut towns: Vec<&Town> = self.towns.iter().filter(|t| t.name.matches(&q)).collect();
        towns.sort_by_key(|t| std::cmp::Reverse(t.population));
        for t in towns.into_iter().take(limit) {
            let region = self.regions.get(&t.region).map(|r| r.get(lang));
            out.push(found(join(lang, t.name.get(lang), region), t, 12.0));
        }
        out.truncate(limit);
        out
    }
}

fn found(label: String, t: &Town, zoom: f64) -> Found {
    Found {
        label,
        latitude: t.latitude,
        longitude: t.longitude,
        zoom,
    }
}

/// 候補に出す「名前（上の地域）」。
fn join(lang: Language, name: &str, within: Option<&str>) -> String {
    match (within, lang) {
        (Some(w), Language::Japanese) => format!("{name}（{w}）"),
        (Some(w), Language::English) => format!("{name}, {w}"),
        (None, _) => name.to_owned(),
    }
}

fn cell(latitude: f64, longitude: f64) -> (i32, i32) {
    (latitude.floor() as i32, longitude.floor() as i32)
}

/// 大円距離（km）。
fn distance_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = p2 - p1;
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * EARTH_KM * a.sqrt().asin()
}

/// 読み込んだ表。データセットの検索カタログを作る処理からも使う。カタログを作る関数は、
/// 取り出し・登録・更新の多くの所から別のスレッドで呼ばれるので、引数で渡して回すより、
/// 読み込んだものを 1 か所に置いて見に行く形にした（中身は読むだけで変えない）。
static CURRENT: RwLock<Option<Arc<Gazetteer>>> = RwLock::new(None);

pub fn current() -> Option<Arc<Gazetteer>> {
    CURRENT.read().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn set_current(g: Option<Arc<Gazetteer>>) {
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = g;
}

/// 撮影地点に付ける地名の、検索カタログでの項目。英語と日本語の両方を入れ、どちらでも探せるようにする。
pub fn catalog_triples(g: &Gazetteer, latitude: f64, longitude: f64) -> Vec<(String, String)> {
    let Some(a) = g.address(latitude, longitude) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, name) in [
        ("Country", a.country),
        ("Region", a.region),
        ("Town", Some(&a.town.name)),
    ] {
        let Some(name) = name else { continue };
        out.push((key.to_owned(), name.en.clone()));
        if !name.ja.is_empty() && name.ja != name.en {
            out.push((key.to_owned(), name.ja.clone()));
        }
    }
    out
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub const SAMPLE: &str = "# vw-geonames 1
C\tJP\tJapan\t日本
C\tFJ\tFiji\tフィジー
A\tJP.12\tHokkaido\t北海道
A\tJP.40\tTokyo\t東京都
A\tFJ.01\tCentral\t
P\t35.68950\t139.69171\tJP\t40\t8336599\tTokyo\t東京
P\t35.69384\t139.70355\tJP\t40\t346235\tShinjuku\t新宿区
P\t43.06417\t141.34694\tJP\t12\t1883027\tSapporo\t札幌市
P\t42.77500\t141.69200\tJP\t12\t90000\tChitose\t千歳市
P\t-18.14161\t178.44149\tFJ\t01\t77366\tSuva\t
";

    fn sample() -> Gazetteer {
        Gazetteer::parse(SAMPLE).unwrap()
    }

    #[test]
    fn photo_gets_the_nearest_town_with_its_region_and_country() {
        let g = sample();
        let a = g.address(43.0621, 141.3544).unwrap();
        assert_eq!(a.text(Language::Japanese), "日本 北海道 札幌市");
        assert_eq!(a.text(Language::English), "Sapporo, Hokkaido, Japan");
    }

    #[test]
    fn nearer_town_wins_over_a_bigger_one() {
        let g = sample();
        let a = g.address(35.6930, 139.7020).unwrap();
        assert_eq!(a.town.name.en, "Shinjuku");
    }

    #[test]
    fn far_from_any_town_has_no_name() {
        assert!(sample().address(40.0, 150.0).is_none());
    }

    #[test]
    fn town_across_the_date_line_is_found() {
        let g = Gazetteer::parse("# vw-geonames 1\nP\t-16.80000\t179.95000\tFJ\t01\t100\tEast\t\n")
            .unwrap();
        assert_eq!(g.address(-16.8, -179.95).unwrap().town.name.en, "East");
    }

    #[test]
    fn english_is_used_when_there_is_no_japanese_name() {
        let g = sample();
        let a = g.address(-18.14, 178.44).unwrap();
        assert_eq!(a.text(Language::Japanese), "フィジー Central Suva");
    }

    #[test]
    fn search_finds_regions_before_towns_and_bigger_towns_first() {
        let g = sample();
        let found = g.search("北海道", Language::Japanese, 10);
        assert_eq!(found[0].label, "北海道（日本）");
        assert!((found[0].latitude - 43.06417).abs() < 1e-9);
        let towns = g.search("s", Language::English, 10);
        let labels: Vec<&str> = towns.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Sapporo, Hokkaido",
                "Shinjuku, Tokyo",
                "Chitose, Hokkaido",
                "Suva, Central"
            ]
        );
    }

    #[test]
    fn search_ignores_case_in_english() {
        assert_eq!(sample().search("TOKYO", Language::English, 10).len(), 2);
    }

    #[test]
    fn catalog_gets_names_in_both_languages() {
        let t = catalog_triples(&sample(), 43.0621, 141.3544);
        assert!(t.contains(&("Region".into(), "Hokkaido".into())));
        assert!(t.contains(&("Region".into(), "北海道".into())));
        assert!(t.contains(&("Town".into(), "札幌市".into())));
        assert!(t.contains(&("Country".into(), "Japan".into())));
    }

    #[test]
    fn other_files_are_rejected() {
        assert!(Gazetteer::parse("hello").is_err());
    }
}
