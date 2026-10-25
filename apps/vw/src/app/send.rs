//! ネットワーク送信のダイアログと送信先の選択。

use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;

use crate::frames;
use crate::i18n::{tr, tr1};
use crate::viewer::SequenceKind;

use super::*;

impl VwApp {
    pub(super) fn send_to(&mut self, choice: crate::send_app::Choice, targets: Vec<PathBuf>) {
        let targets = targets
            .into_iter()
            .map(crate::dicom_send::Target::Path)
            .collect();
        self.send_targets(choice, targets);
    }

    /// 「送る ▸」で選ばれた送り先へ `targets` を送る。
    pub(super) fn send_targets(
        &mut self,
        choice: crate::send_app::Choice,
        targets: Vec<crate::dicom_send::Target>,
    ) {
        use crate::send_app::Choice;
        match choice {
            Choice::Network(choice) => self.send_to_network(choice, targets),
            Choice::SetUpApps => self.open_settings_at(|s| s.show_send_apps()),
            Choice::OpenDefault | Choice::App(_) => {
                let paths = self.launch_paths(targets);
                self.launch(choice, paths);
            }
        }
    }

    /// 設定を開いて、`jump` の節へ移る。
    fn open_settings_at(&mut self, jump: impl FnOnce(&mut crate::settings_ui::SettingsUi)) {
        self.show_fullscreen_setup = false;
        if !self.show_settings {
            self.toggle_settings();
        }
        jump(&mut self.settings_ui);
    }

    /// アプリに渡すパス。ZIP・7z の中の 1 ファイル（データセットの検索で当たったもの）は、
    /// 展開していないので渡せない。
    fn launch_paths(&mut self, targets: Vec<crate::dicom_send::Target>) -> Vec<PathBuf> {
        use crate::dicom_send::Target;
        let mut skipped = 0;
        let paths = targets
            .into_iter()
            .filter_map(|t| match t {
                Target::Path(p) => Some(p),
                Target::Member { .. } => {
                    skipped += 1;
                    None
                }
            })
            .collect();
        if skipped > 0 {
            self.log.warn(format!(
                "Send to app: skipped {skipped} file(s) inside ZIP/7z (open the archive to send them)"
            ));
        }
        paths
    }

    /// 送り先のアプリの一覧。
    pub(super) fn send_apps(&self) -> &[crate::settings::SendApp] {
        self.settings.settings.send.apps.as_deref().unwrap_or(&[])
    }

    /// キーの「送り先のアプリ n」。見ているスロットの画像を送る。
    pub(super) fn send_active_to_app(&mut self, i: usize) {
        let lang = self.lang();
        let paths = self.slot_send_targets(self.active);
        let Some(first) = paths.first() else {
            return;
        };
        let Some(app) = self.send_apps().get(i) else {
            self.log.warn(tr1(
                lang,
                "There is no send-to app number {}",
                &(i + 1).to_string(),
            ));
            return;
        };
        if !app.takes(false, crate::send_app::extension(first).as_deref()) {
            self.log.warn(format!(
                "Send to {}: does not take {}",
                app.name,
                first.display()
            ));
            return;
        }
        self.launch(crate::send_app::Choice::App(i), paths);
    }

    /// アプリを起動する。起動の回数が多ければ、先に確かめる。
    fn launch(&mut self, choice: crate::send_app::Choice, paths: Vec<PathBuf>) {
        use crate::send_app::{Choice, Launch, CONFIRM_LAUNCHES};
        if paths.is_empty() {
            return;
        }
        let launch = match choice {
            Choice::OpenDefault => Launch::Default(paths.clone()),
            Choice::App(i) => match self.send_apps().get(i) {
                Some(app) => Launch::app(app, &paths),
                None => return,
            },
            Choice::SetUpApps | Choice::Network(_) => return,
        };
        // vw を閉じると、ZIP を展開した一時フォルダは消える。渡した先で後から開き直すと見つからない。
        if paths.iter().any(|p| self.archives.mount_of(p).is_some()) {
            self.log.warn(
                "Send to app: files inside a ZIP are passed from a temporary folder, which is deleted when vw closes",
            );
        }
        if launch.count() >= CONFIRM_LAUNCHES {
            self.launch_question = Some(launch);
        } else {
            self.run_launch(launch);
        }
    }

    fn run_launch(&mut self, launch: crate::send_app::Launch) {
        let lang = self.lang();
        let name = launch.name(lang);
        let count = launch.count();
        let failures = launch.run();
        for failure in &failures {
            self.log.error(format!("Send to {name}: {failure}"));
        }
        if failures.len() < count {
            self.log.info(format!(
                "Sent to {name} ({} launch(es))",
                count - failures.len()
            ));
        }
    }

    /// アプリをたくさん起動する前の確かめ。
    pub(super) fn launch_ui(&mut self, ctx: &egui::Context) {
        let lang = self.lang();
        let Some(launch) = &self.launch_question else {
            return;
        };
        let mut answer = None;
        let modal = egui::Modal::new(egui::Id::new("launch_question")).show(ctx, |ui| {
            ui.set_max_width(520.0);
            ui.heading(tr(lang, "Send"));
            ui.add_space(4.0);
            ui.label(
                tr(lang, "Start {app} {n} times?")
                    .replace("{app}", &launch.name(lang))
                    .replace("{n}", &thousands(launch.count())),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(tr(lang, "Launch")).clicked() {
                    answer = Some(true);
                }
                if ui.button(tr(lang, "Cancel")).clicked() {
                    answer = Some(false);
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                if let Some(launch) = self.launch_question.take() {
                    self.run_launch(launch);
                }
            }
            Some(false) => self.launch_question = None,
            None => {}
        }
    }

    /// 送り先のアプリをまだ決めていなければ、OS の関連付けから作った一覧を入れる。
    /// 関連付けは裏で調べ、起動を待たせない。
    pub(super) fn fill_send_apps(&mut self, ctx: &egui::Context) {
        if self.settings.settings.send.apps.is_some() {
            return;
        }
        if let Some(found) = &self.associated_apps {
            self.settings.settings.send.apps = Some(found.clone());
            return;
        }
        let rx = self.associated_rx.get_or_insert_with(|| {
            crate::background::compute(ctx, || {
                vw_launch::assoc::associated_apps(&crate::send_app::ASSOCIATED_EXTENSIONS)
                    .into_iter()
                    .map(crate::send_app::from_associated)
                    .collect()
            })
        });
        if let Ok(found) = rx.try_recv() {
            self.associated_rx = None;
            self.associated_apps = Some(found);
        }
    }

    fn send_to_network(
        &mut self,
        choice: crate::dicom_send::Choice,
        targets: Vec<crate::dicom_send::Target>,
    ) {
        use crate::dicom_send::{Choice, Job};
        let lang = self.lang();
        let i = match choice {
            Choice::Configure => {
                self.open_settings_at(|s| s.show_destinations());
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
