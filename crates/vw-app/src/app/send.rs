//! ネットワーク送信のダイアログと送信先の選択。

use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;

use crate::frames;
use crate::i18n::{tr, tr1};
use crate::viewer::SequenceKind;

use super::*;

impl VwApp {
    pub(super) fn send_to(&mut self, choice: crate::dicom_send::Choice, targets: Vec<PathBuf>) {
        let targets = targets
            .into_iter()
            .map(crate::dicom_send::Target::Path)
            .collect();
        self.send_targets(choice, targets);
    }

    pub(super) fn send_targets(
        &mut self,
        choice: crate::dicom_send::Choice,
        targets: Vec<crate::dicom_send::Target>,
    ) {
        use crate::dicom_send::{Choice, Job};
        let lang = self.lang();
        let i = match choice {
            Choice::Configure => {
                self.show_datasets = false;
                self.show_fullscreen_setup = false;
                if !self.show_settings {
                    self.toggle_settings();
                }
                self.settings_ui.show_destinations();
                return;
            }
            Choice::To(i) => i,
        };
        if self.send_job.as_ref().is_some_and(|j| j.sending) {
            self.log.warn(tr(
                lang,
                "Already sending. Wait for it to finish or cancel it first.",
            ));
            return;
        }
        let send = &self.settings.settings.dicom_send;
        let Some(destination) = send.destinations.get(i).cloned() else {
            return;
        };
        if let Some(mut old) = self.send_job.take() {
            old.cancel();
        }
        self.send_job = Some(Job::new(
            destination,
            send.ae_title.clone(),
            send.timeout(),
            targets,
        ));
    }

    /// スロットから送るファイル。シリーズのスロットはシリーズの全ファイル、
    /// マルチフレームはフレームではなくファイル 1 つ。
    pub(super) fn slot_send_targets(&self, index: usize) -> Vec<PathBuf> {
        let slot = &self.slots[index];
        if let Some(seq) = slot
            .sequence
            .as_ref()
            .filter(|q| q.kind == SequenceKind::Series)
        {
            return seq.paths.clone();
        }
        slot.shown
            .as_ref()
            .map(|s| vec![frames::file_of(&s.path)])
            .unwrap_or_default()
    }

    /// 送信の知らせをログに出し、送る前の確認を出す。
    pub(super) fn send_ui(&mut self, ctx: &egui::Context) {
        use crate::dicom_send::Event;
        let lang = self.lang();
        let Some(job) = &mut self.send_job else {
            return;
        };
        let name = format!("{} ({})", job.destination.name, job.destination.address());
        let mut finished = false;
        for event in job.poll() {
            match event {
                Event::Counted { total, notes } => {
                    for note in notes {
                        self.log.warn(format!("Send: {note}"));
                    }
                    if total == 0 {
                        self.log
                            .warn(format!("Send to {name}: no DICOM files found"));
                    }
                }
                Event::Failed { file, reason } => {
                    self.log.error(format!("Send to {name}: {file}: {reason}"));
                }
                Event::Warning { file, status } => {
                    self.log.warn(format!(
                        "Send to {name}: {file}: stored with warning {status:04X}H"
                    ));
                }
                Event::Finished {
                    sent,
                    failed,
                    cancelled,
                    stopped,
                } => {
                    finished = true;
                    let line = format!("Send to {name}: sent {sent}, failed {failed}");
                    match (stopped, cancelled) {
                        (Some(e), _) => self.log.error(format!("{line} (stopped: {e})")),
                        (None, true) => self.log.warn(format!("{line} (cancelled)")),
                        (None, false) if failed > 0 => self.log.error(line),
                        (None, false) => self.log.done("Sent", format!("{sent} file(s) to {name}")),
                    }
                }
            }
        }
        if finished {
            self.send_job = None;
            return;
        }
        let Some(job) = &mut self.send_job else {
            return;
        };
        if job.sending {
            ctx.request_repaint_after(Duration::from_millis(200));
            return;
        }
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::new("send_question")).show(ctx, |ui| {
            ui.set_max_width(520.0);
            ui.heading(tr(lang, "Send"));
            ui.add_space(4.0);
            match job.total {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tr1(
                            lang,
                            "Looking for DICOM files... {}",
                            &thousands(job.counted()),
                        ));
                    });
                }
                Some(0) => {
                    ui.label(tr(lang, "There are no DICOM files to send."));
                }
                Some(n) => {
                    ui.label(
                        tr(lang, "Send {n} files to {destination}?")
                            .replace("{n}", &thousands(n))
                            .replace("{destination}", &name),
                    );
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let ready = job.total.is_some_and(|n| n > 0);
                if ready && ui.button(tr(lang, "Send")).clicked() {
                    answer = Some(true);
                }
                let close = if job.total == Some(0) {
                    "Close"
                } else {
                    "Cancel"
                };
                if ui.button(tr(lang, close)).clicked() {
                    answer = Some(false);
                }
            });
        });
        if job.total.is_none() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                let n = job.total.unwrap_or(0);
                self.log
                    .info(format!("Sending {} files to {name}", thousands(n)));
                job.start();
            }
            Some(false) => {
                job.cancel();
                self.send_job = None;
            }
            None => {}
        }
    }

    /// 送信を取り消す。
    pub(super) fn cancel_send(&mut self) {
        if let Some(job) = &mut self.send_job {
            job.cancel();
        }
    }
}
