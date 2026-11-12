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
}

/// 裏で読む頼み。
#[derive(Debug, Clone, PartialEq)]
struct Request {
    file: PathBuf,
    frame: usize,
    points: Vec<(u32, u32)>,
    flip_rows: bool,
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
            let mut open: Option<(PathBuf, bool, vw_fits::cube::Cube)> = None;
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
    open: &mut Option<(PathBuf, bool, vw_fits::cube::Cube)>,
    request: &Request,
    choices: &vw_fits::Choices,
) -> Found {
    let fresh = open
        .as_ref()
        .is_some_and(|(f, flip, _)| *f == request.file && *flip == request.flip_rows);
    let mut problem = None;
    if !fresh {
        *open = match std::fs::read(&request.file) {
            Ok(bytes) => {
                let chosen = choices.get(&bytes);
                match vw_fits::cube::Cube::new(&bytes, chosen, request.flip_rows) {
                    Ok(cube) => Some((request.file.clone(), request.flip_rows, cube)),
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
    if let Some((_, _, cube)) = open {
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
        let marker = (request.frame >= first.frame_of(0)).then(|| {
            (request.frame - first.frame_of(0))
                / first.frame_of(1).saturating_sub(first.frame_of(0)).max(1)
        });
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click());
        let hover = response.hover_pos();
        let painter = ui.painter_at(rect);
        let hovered =
            spectrum_view::paint_series(&painter, rect, &series, marker, lang, ui.visuals(), hover);
        if response.clicked()
            && let Some(channel) = hovered
        {
            let frame = first.frame_of(channel);
            let file = request.file.clone();
            self.jump_to_frame(self.active, &file, frame);
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
