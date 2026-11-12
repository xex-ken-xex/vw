//! 十字のスペクトルの欄。アクティブなスロットの FITS のキューブに置いた十字ごとに、その点の
//! 全部の面の値を折れ線で重ねる。十字を動かすと追い、グラフをクリックするとその面に飛ぶ。
//!
//! 点の値は裏のスレッドで読む。大きなキューブはファイルを読むだけで時間がかかり、
//! タイル圧縮では面を戻すのにも時間がかかるので、描く手を止めないため。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui::{self, Sense};

use crate::i18n::tr;
use crate::spectrum_view::{self, Series};
use crate::viewer::CROSS_COLORS;

use super::VwApp;

#[derive(Default)]
pub(super) struct PointSpectrumPane {
    pub open: bool,
    worker: Option<Worker>,
    /// 最後に頼んだもの。同じなら頼み直さない。
    asked: Option<Request>,
    /// 最後に届いたもの。
    found: Option<Found>,
    /// グラフをドラッグして選んだチャンネルの範囲（キューブのファイル、両端）。モーメント図を
    /// 作る窓の初めの範囲にする。
    pub range: Option<(PathBuf, usize, usize)>,
    /// ドラッグを始めたチャンネル。
    drag_from: Option<usize>,
}

/// 裏で読む頼み。
#[derive(Debug, Clone, PartialEq)]
struct Request {
    file: PathBuf,
    frame: usize,
    points: Vec<(u32, u32)>,
    flip_rows: bool,
    /// 見ている HDU の名前（メタデータの `HDU`）。同じファイルで HDU を切り替えたら読み直すため。
    hdu: Option<String>,
}

/// 開いたキューブが、どのファイルのどの読み方か。どれかが変われば開き直す。
#[derive(Debug, Clone, PartialEq)]
struct Opened {
    file: PathBuf,
    flip_rows: bool,
    hdu: Option<String>,
    modified: Option<std::time::SystemTime>,
}

struct Found {
    request: Request,
    points: Vec<Option<vw_fits::cube::Point>>,
    problem: Option<String>,
}

struct Worker {
    tx: Sender<Request>,
    rx: Receiver<Found>,
}

impl Worker {
    fn start(choices: vw_fits::Choices, ctx: egui::Context) -> Self {
        let (tx, requests) = channel::<Request>();
        let (answers, rx) = channel();
        std::thread::spawn(move || {
            let mut open: Option<(Opened, vw_fits::cube::Cube)> = None;
            while let Ok(mut request) = requests.recv() {
                // 十字をドラッグしている間に溜まった頼みは、最後の 1 つだけ読む。
                while let Ok(newer) = requests.try_recv() {
                    request = newer;
                }
                let found = read(&mut open, &request, &choices);
                if answers.send(found).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        Self { tx, rx }
    }
}

fn read(
    open: &mut Option<(Opened, vw_fits::cube::Cube)>,
    request: &Request,
    choices: &vw_fits::Choices,
) -> Found {
    let wanted = Opened {
        file: request.file.clone(),
        flip_rows: request.flip_rows,
        hdu: request.hdu.clone(),
        modified: std::fs::metadata(&request.file)
            .and_then(|m| m.modified())
            .ok(),
    };
    let fresh = open.as_ref().is_some_and(|(o, _)| *o == wanted);
    let mut problem = None;
    if !fresh {
        *open = match std::fs::read(&request.file) {
            Ok(bytes) => {
                let chosen = choices.get(&bytes);
                match vw_fits::cube::Cube::new(&bytes, chosen, request.flip_rows) {
                    Ok(cube) => Some((wanted, cube)),
                    Err(e) => {
                        problem = Some(e.to_string());
                        None
                    }
                }
            }
            Err(e) => {
                problem = Some(e.to_string());
                None
            }
        };
    }
    let mut points = Vec::new();
    if let Some((_, cube)) = open {
        for &(x, y) in &request.points {
            match cube.spectrum(request.frame, x, y) {
                Ok(p) => points.push(p),
                Err(e) => {
                    problem = Some(e.to_string());
                    points.push(None);
                }
            }
        }
    }
    Found {
        request: request.clone(),
        points,
        problem,
    }
}

impl VwApp {
    pub(super) fn point_spectrum_pane(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let Some(request) = self.point_spectrum_request() else {
            self.point_spectrum.found = None;
            ui.weak(tr(
                lang,
                "Place crosses with the pixel probe on a FITS cube to see the values of those points on every plane.",
            ));
            return;
        };
        let pane = &mut self.point_spectrum;
        let worker = pane
            .worker
            .get_or_insert_with(|| Worker::start(self.fits_choices.clone(), ui.ctx().clone()));
        while let Ok(found) = worker.rx.try_recv() {
            pane.found = Some(found);
        }
        if pane.asked.as_ref() != Some(&request) {
            if worker.tx.send(request.clone()).is_err() {
                pane.worker = None;
            }
            pane.asked = Some(request.clone());
        }
        // 同じファイルの前の答えは、新しい答えが届くまで見せ続ける（十字を動かすたびに点滅しない）。
        let Some(found) = pane
            .found
            .as_ref()
            .filter(|f| f.request.file == request.file)
        else {
            ui.weak(tr(lang, "Reading the cube..."));
            return;
        };
        if let Some(problem) = &found.problem {
            ui.colored_label(ui.visuals().error_fg_color, problem);
        }
        let series: Vec<Series> = found
            .points
            .iter()
            .enumerate()
            .filter_map(|(n, p)| {
                Some(Series {
                    spectrum: &p.as_ref()?.spectrum,
                    color: CROSS_COLORS[n % CROSS_COLORS.len()],
                })
            })
            .collect();
        let Some(first) = found.points.iter().flatten().next() else {
            ui.weak(tr(lang, "This file has no planes to follow."));
            return;
        };
        // 届いた答えが前の面のものでも、縦線は表示中の面に引く。
        let plane = (request.frame >= first.frame_of(0)).then(|| {
            (request.frame - first.frame_of(0))
                / first.frame_of(1).saturating_sub(first.frame_of(0)).max(1)
        });
        let band = pane
            .range
            .as_ref()
            .filter(|(f, _, _)| *f == request.file)
            .map(|&(_, a, b)| (a, b));
        // マウスを乗せたときの説明にすると、値の読み取りの上に重なって軸が読めなくなるので、上に出す。
        ui.weak(tr(
            lang,
            "Click to show that plane. Drag to choose the channels for moment maps; double-click to clear them.",
        ));
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let hover = response.interact_pointer_pos().or(response.hover_pos());
        let painter = ui.painter_at(rect);
        let marks = spectrum_view::Marks { plane, band };
        let hovered =
            spectrum_view::paint_series(&painter, rect, &series, marks, lang, ui.visuals(), hover);
        let jump = hovered.map(|c| first.frame_of(c));
        if response.drag_started() {
            pane.drag_from = hovered;
        }
        if response.dragged()
            && let (Some(from), Some(to)) = (pane.drag_from, hovered)
        {
            pane.range = Some((request.file.clone(), from.min(to), from.max(to)));
        }
        if response.drag_stopped() {
            pane.drag_from = None;
        }
        if response.double_clicked() {
            pane.range = None;
        } else if response.clicked()
            && let Some(frame) = jump
        {
            self.jump_to_frame(self.active, &request.file, frame);
        }
    }

    /// アクティブなスロットのキューブと、置いた十字。キューブでなければ `None`。
    fn point_spectrum_request(&self) -> Option<Request> {
        let slot = self.slots.get(self.active)?;
        let shown = slot.shown()?;
        if !crate::viewer::is_fits_cube(&shown.info) {
            return None;
        }
        let key = shown.mark_path();
        let points: Vec<(u32, u32)> = slot
            .marks
            .iter()
            .filter(|m| m.path == key)
            .map(|m| (m.x, m.y))
            .collect();
        if points.is_empty() {
            return None;
        }
        // 1 面目はフレームの印のないファイルのパスで出していることがある。
        let (file, frame) =
            crate::frames::split(&shown.path).unwrap_or_else(|| (shown.path.clone(), 0));
        Some(Request {
            file,
            frame,
            points,
            flip_rows: self.fits.get().flip_rows,
            hdu: shown
                .info
                .metadata
                .get(vw_fits::GROUP, vw_fits::HDU)
                .map(str::to_owned),
        })
    }

    /// スロット `index` に、キューブ `file` の面 `frame` を出す。表示倍率と位置はそのまま。
    pub(super) fn jump_to_frame(&mut self, index: usize, file: &Path, frame: usize) {
        let Some(slot) = self.slots.get_mut(index) else {
            return;
        };
        let path = match &mut slot.sequence {
            Some(seq)
                if seq.kind == crate::viewer::SequenceKind::Frames
                    && seq.folder == file
                    && frame < seq.paths.len() =>
            {
                seq.index = frame;
                seq.paths[frame].clone()
            }
            _ => crate::frames::frame_path(file, frame),
        };
        let full = self.cell_needs_full();
        self.start_load(index, path, full);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 80 文字のカードを並べた HDU。データは `data`。
    fn hdu(cards: &[(&str, &str)], data: &[u8]) -> Vec<u8> {
        let mut text = String::new();
        for (k, v) in cards {
            text.push_str(&format!("{:<80}", format!("{k:<8}= {v:>20}")));
        }
        text.push_str(&format!("{:<80}", "END"));
        let mut out = text.into_bytes();
        out.resize(out.len().next_multiple_of(2880), b' ');
        out.extend_from_slice(data);
        out.resize(out.len().next_multiple_of(2880), 0);
        out
    }

    /// 1 × 1 画素 × 3 面のキューブを 2 つ持つファイル。HDU 1 は値 1、HDU 2 は値 2。
    fn two_cubes() -> Vec<u8> {
        let mut file = hdu(&[("SIMPLE", "T"), ("BITPIX", "8"), ("NAXIS", "0")], &[]);
        for value in [1u8, 2] {
            file.extend(hdu(
                &[
                    ("XTENSION", "'IMAGE   '"),
                    ("BITPIX", "8"),
                    ("NAXIS", "3"),
                    ("NAXIS1", "1"),
                    ("NAXIS2", "1"),
                    ("NAXIS3", "3"),
                ],
                &[value; 3],
            ));
        }
        file
    }

    #[test]
    fn switching_to_another_hdu_of_the_same_file_reads_that_hdu() {
        let bytes = two_cubes();
        let file = std::env::temp_dir().join(format!("vw-point-{}.fits", std::process::id()));
        std::fs::write(&file, &bytes).unwrap();
        let choices = vw_fits::Choices::default();
        let mut open = None;
        let mut request = Request {
            file: file.clone(),
            frame: 0,
            points: vec![(0, 0)],
            flip_rows: false,
            hdu: Some("1".into()),
        };
        let value = |found: &Found| found.points[0].as_ref().unwrap().spectrum.values[0];
        assert_eq!(value(&read(&mut open, &request, &choices)), 1.0);
        choices.set(&bytes, 2usize);
        request.hdu = Some("2".into());
        let found = read(&mut open, &request, &choices);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(value(&found), 2.0);
    }
}
