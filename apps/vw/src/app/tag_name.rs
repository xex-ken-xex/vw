//! タグのない数字キーを押したときに、その場でタグの名前を入れる小窓。
//!
//! 前はログに「設定で足して」と出るだけで、押しても何も起きないように見えた。設定の画面へ
//! 飛ばさずにここで聞くのは、タグを付けようとしている流れを切らないため。

use eframe::egui;

use crate::i18n::{tr, tr1};
use crate::settings::Labels;

use super::*;

/// 名前を聞いている途中。
pub(super) struct TagNamePrompt {
    /// 押された数字キー（1〜9）。
    key: usize,
    input: String,
    focused: bool,
}

impl TagNamePrompt {
    pub(super) fn new(key: usize) -> Self {
        Self {
            key,
            input: String::new(),
            focused: false,
        }
    }
}

/// 入れた名前を足せるか。足せなければ、その理由（英語の文言）。
fn check_name(tags: &[String], name: &str) -> Result<String, &'static str> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a name.");
    }
    if name.contains(',') {
        return Err("A tag name cannot contain a comma.");
    }
    let mut next = tags.to_vec();
    next.push(name.to_owned());
    if !Labels::valid_tags(&next) {
        return Err("Tag names must be unique.");
    }
    Ok(name.to_owned())
}

impl VwApp {
    pub(super) fn tag_name_dialog(&mut self, ctx: &egui::Context) {
        let Some(prompt) = &mut self.tag_name_prompt else {
            return;
        };
        let lang = self.settings.settings.appearance.language;
        let tags = &self.settings.settings.labels.tags;
        // タグは前から順に数字キーへ並ぶので、足すタグは押したキーでなく空いている次のキーになる。
        let next_key = tags.len() + 1;
        let checked = check_name(tags, &prompt.input);
        let mut answer = None;
        let modal = vw_ui::dialog::Dialog::new("tag_name")
            .max_width(420.0)
            .show(ctx, |ui| {
                ui.label(tr1(
                    lang,
                    "No tag is set for key {}.",
                    &prompt.key.to_string(),
                ));
                if next_key == prompt.key {
                    ui.label(tr(lang, "Name a new tag to put on this image:"));
                } else {
                    // 前のキーが空いていないと、足したタグは押したキーでなく次の空いたキーに並ぶ。
                    ui.label(tr1(
                        lang,
                        "Name a new tag for key {} and put it on this image:",
                        &next_key.to_string(),
                    ));
                }
                ui.add_space(4.0);
                let field = ui.add(
                    egui::TextEdit::singleline(&mut prompt.input)
                        .hint_text(tr(lang, "e.g. cat"))
                        .desired_width(f32::INFINITY),
                );
                // 毎フレーム focus を求めると、ボタンのクリックまで取られてしまうので、開いたときだけ。
                if !prompt.focused {
                    field.request_focus();
                    prompt.focused = true;
                }
                if let Err(why) = &checked
                    && !prompt.input.is_empty()
                {
                    ui.colored_label(ui.visuals().error_fg_color, tr(lang, why));
                }
                let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.weak(tr(
                    lang,
                    "Tags can also be edited in Settings > Flags and tags.",
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let add =
                        ui.add_enabled(checked.is_ok(), egui::Button::new(tr(lang, "Add tag")));
                    if add.clicked() || (entered && checked.is_ok()) {
                        answer = Some(true);
                    }
                    if ui.button(tr(lang, "Cancel")).clicked() {
                        answer = Some(false);
                    }
                });
            });
        if modal.should_close && answer.is_none() {
            answer = Some(false);
        }
        let Some(add) = answer else {
            return;
        };
        self.tag_name_prompt = None;
        let Some(name) = checked.ok().filter(|_| add) else {
            return;
        };
        self.settings.settings.labels.tags.push(name.clone());
        self.settings_changed(ctx);
        self.log
            .info(format!("Tags: added {name} for key {next_key}"));
        self.tag(next_key - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_new_name_is_trimmed() {
        assert_eq!(check_name(&tags(&["cat"]), "  dog "), Ok("dog".to_owned()));
    }

    #[test]
    fn empty_duplicate_and_comma_names_are_refused() {
        let have = tags(&["cat"]);
        assert!(check_name(&have, " ").is_err());
        assert!(check_name(&have, "cat").is_err());
        assert!(check_name(&have, "dog, bird").is_err());
    }
}
