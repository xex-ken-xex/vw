//! ホットキー。操作（[`Action`]）とキーの組み合わせ（[`KeyCombo`]）の対応を持つ。
//!
//! 設定ファイルには `next_image = ["Right", "PageDown", "Space"]` のように
//! 操作名とキーの文字列で保存する。

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use eframe::egui::{Event, Key, Modifiers};
use serde::Serialize;

use crate::layout::SlotLayout;

/// ホットキーを割り当てられる操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    PreviousImage,
    NextImage,
    NewSlot,
    CloseSlot,
    ToggleFolderTree,
    ToggleMetadata,
    ToggleImages,
    ToggleLog,
    ToggleSettings,
    CloseAllSlots,
    Quit,
    PreviousSlot,
    NextSlot,
    PreviousPage,
    NextPage,
    ToggleMaximize,
    FitImage,
    ActualSize,
    ZoomIn,
    ZoomOut,
    LayoutAuto,
    Layout1x1,
    Layout1x2,
    Layout2x1,
    Layout2x2,
    Layout3x3,
    Layout4x4,
    LayoutCustom,
    TogglePlay,
}

impl Action {
    /// 設定画面に並べる順。
    pub const ALL: [Action; 29] = [
        Action::PreviousImage,
        Action::NextImage,
        Action::PreviousSlot,
        Action::NextSlot,
        Action::PreviousPage,
        Action::NextPage,
        Action::ToggleMaximize,
        Action::TogglePlay,
        Action::FitImage,
        Action::ActualSize,
        Action::ZoomIn,
        Action::ZoomOut,
        Action::LayoutAuto,
        Action::Layout1x1,
        Action::Layout1x2,
        Action::Layout2x1,
        Action::Layout2x2,
        Action::Layout3x3,
        Action::Layout4x4,
        Action::LayoutCustom,
        Action::NewSlot,
        Action::CloseSlot,
        Action::CloseAllSlots,
        Action::Quit,
        Action::ToggleFolderTree,
        Action::ToggleMetadata,
        Action::ToggleImages,
        Action::ToggleLog,
        Action::ToggleSettings,
    ];

    /// 設定ファイルでの名前。
    pub fn id(self) -> &'static str {
        match self {
            Action::PreviousImage => "previous_image",
            Action::NextImage => "next_image",
            Action::NewSlot => "new_slot",
            Action::CloseSlot => "close_slot",
            Action::ToggleFolderTree => "toggle_folder_tree",
            Action::ToggleMetadata => "toggle_metadata",
            Action::ToggleImages => "toggle_images",
            Action::ToggleLog => "toggle_log",
            Action::ToggleSettings => "toggle_settings",
            Action::CloseAllSlots => "close_all_slots",
            Action::Quit => "quit",
            Action::PreviousSlot => "previous_slot",
            Action::NextSlot => "next_slot",
            Action::PreviousPage => "previous_page",
            Action::NextPage => "next_page",
            Action::ToggleMaximize => "toggle_maximize",
            Action::FitImage => "fit_image",
            Action::ActualSize => "actual_size",
            Action::ZoomIn => "zoom_in",
            Action::ZoomOut => "zoom_out",
            Action::LayoutAuto => "layout_auto",
            Action::Layout1x1 => "layout_1x1",
            Action::Layout1x2 => "layout_1x2",
            Action::Layout2x1 => "layout_2x1",
            Action::Layout2x2 => "layout_2x2",
            Action::Layout3x3 => "layout_3x3",
            Action::Layout4x4 => "layout_4x4",
            Action::LayoutCustom => "layout_custom",
            Action::TogglePlay => "toggle_play",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }

    /// 画面に出す名前（英語。翻訳は [`crate::i18n`]）。
    pub fn title(self) -> &'static str {
        match self {
            Action::PreviousImage => "Previous image",
            Action::NextImage => "Next image",
            Action::NewSlot => "New slot",
            Action::CloseSlot => "Close slot",
            Action::ToggleFolderTree => "Show/hide folder tree",
            Action::ToggleMetadata => "Show/hide metadata",
            Action::ToggleImages => "Show/hide images",
            Action::ToggleLog => "Show/hide log",
            Action::ToggleSettings => "Open/close settings",
            Action::CloseAllSlots => "Close all slots",
            Action::Quit => "Quit",
            Action::PreviousSlot => "Previous slot",
            Action::NextSlot => "Next slot",
            Action::PreviousPage => "Previous page",
            Action::NextPage => "Next page",
            Action::ToggleMaximize => "Maximize/restore slot",
            Action::FitImage => "Fit image to slot",
            Action::ActualSize => "Actual size (100%)",
            Action::ZoomIn => "Zoom in",
            Action::ZoomOut => "Zoom out",
            Action::LayoutAuto => "Layout: auto",
            Action::Layout1x1 => "Layout: 1 x 1",
            Action::Layout1x2 => "Layout: 1 x 2",
            Action::Layout2x1 => "Layout: 2 x 1",
            Action::Layout2x2 => "Layout: 2 x 2",
            Action::Layout3x3 => "Layout: 3 x 3",
            Action::Layout4x4 => "Layout: 4 x 4",
            Action::LayoutCustom => "Layout: custom",
            Action::TogglePlay => "Play/pause slideshow",
        }
    }

    /// キーを押し続けたときの自動リピートで繰り返すか。
    /// 表示切替まで繰り返すと、押している間パネルが点滅してしまう。
    fn repeats(self) -> bool {
        matches!(
            self,
            Action::PreviousImage
                | Action::NextImage
                | Action::PreviousSlot
                | Action::NextSlot
                | Action::PreviousPage
                | Action::NextPage
                | Action::ZoomIn
                | Action::ZoomOut
        )
    }

    /// メニューのレイアウト項目が選ぶマス目。`LayoutCustom` は設定の値を使うのでここにはない。
    pub fn layout(self) -> Option<SlotLayout> {
        let [auto, l1x1, l1x2, l2x1, l2x2, l3x3, l4x4] = SlotLayout::PRESETS;
        match self {
            Action::LayoutAuto => Some(auto),
            Action::Layout1x1 => Some(l1x1),
            Action::Layout1x2 => Some(l1x2),
            Action::Layout2x1 => Some(l2x1),
            Action::Layout2x2 => Some(l2x2),
            Action::Layout3x3 => Some(l3x3),
            Action::Layout4x4 => Some(l4x4),
            _ => None,
        }
    }

    /// 以前の版の既定のキー。設定ファイルにこれが残っていれば今の既定のキーに読み替える。
    /// 読み替えないと、新しい操作の既定のキーと重なって、毎回起動時に赤いログが出てしまう。
    fn legacy_default_keys(self) -> Option<Vec<KeyCombo>> {
        use Key::*;
        match self {
            // PageUp / PageDown はページ送りに移した。
            Action::PreviousImage => {
                Some(vec![KeyCombo::plain(ArrowLeft), KeyCombo::plain(PageUp)])
            }
            Action::NextImage => Some(vec![
                KeyCombo::plain(ArrowRight),
                KeyCombo::plain(PageDown),
                KeyCombo::plain(Space),
            ]),
            _ => None,
        }
    }

    fn default_keys(self) -> Vec<KeyCombo> {
        use Key::*;
        match self {
            Action::PreviousImage => vec![KeyCombo::plain(ArrowLeft)],
            Action::NextImage => vec![KeyCombo::plain(ArrowRight), KeyCombo::plain(Space)],
            Action::NewSlot => vec![KeyCombo::ctrl(N)],
            Action::CloseSlot => vec![KeyCombo::ctrl(W)],
            Action::ToggleFolderTree => vec![KeyCombo::ctrl(Num1)],
            Action::ToggleMetadata => vec![KeyCombo::ctrl(Num2)],
            Action::ToggleImages => vec![KeyCombo::ctrl(Num3)],
            Action::ToggleLog => vec![KeyCombo::ctrl(Num4)],
            Action::ToggleSettings => vec![KeyCombo::ctrl(Comma)],
            Action::CloseAllSlots => vec![KeyCombo::ctrl(W).with(true, true, false)],
            Action::Quit => vec![KeyCombo::ctrl(Q)],
            Action::PreviousSlot => vec![KeyCombo::ctrl(ArrowLeft)],
            Action::NextSlot => vec![KeyCombo::ctrl(ArrowRight)],
            Action::PreviousPage => vec![KeyCombo::plain(PageUp)],
            Action::NextPage => vec![KeyCombo::plain(PageDown)],
            Action::ToggleMaximize => vec![KeyCombo::plain(Enter)],
            Action::FitImage => vec![KeyCombo::plain(Num0)],
            Action::ActualSize => vec![KeyCombo::ctrl(Num0)],
            Action::ZoomIn => vec![KeyCombo::plain(Plus), KeyCombo::plain(Equals)],
            Action::ZoomOut => vec![KeyCombo::plain(Minus)],
            Action::LayoutAuto => vec![KeyCombo::alt(Num0)],
            Action::Layout1x1 => vec![KeyCombo::alt(Num1)],
            Action::Layout1x2 => vec![KeyCombo::alt(Num2)],
            Action::Layout2x1 => vec![KeyCombo::alt(Num3)],
            Action::Layout2x2 => vec![KeyCombo::alt(Num4)],
            Action::Layout3x3 => vec![KeyCombo::alt(Num5)],
            Action::Layout4x4 => vec![KeyCombo::alt(Num6)],
            Action::LayoutCustom => vec![KeyCombo::alt(Num7)],
            Action::TogglePlay => vec![KeyCombo::plain(P)],
        }
    }
}

/// 修飾キーとキー 1 つの組み合わせ。
///
/// Windows キー（Super）は持たない。egui がこの修飾キーを伝えてこないうえ、
/// Windows キーの組み合わせはほぼすべて OS が先に使ってしまうため。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: Key,
}

impl KeyCombo {
    pub const fn plain(key: Key) -> Self {
        Self {
            ctrl: false,
            shift: false,
            alt: false,
            key,
        }
    }

    pub const fn ctrl(key: Key) -> Self {
        Self {
            ctrl: true,
            ..Self::plain(key)
        }
    }

    pub const fn alt(key: Key) -> Self {
        Self {
            alt: true,
            ..Self::plain(key)
        }
    }

    pub fn from_event(key: Key, modifiers: Modifiers) -> Self {
        Self {
            ctrl: modifiers.ctrl,
            shift: modifiers.shift,
            alt: modifiers.alt,
            key,
        }
    }

    const fn with(self, ctrl: bool, shift: bool, alt: bool) -> Self {
        Self {
            ctrl,
            shift,
            alt,
            key: self.key,
        }
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        f.write_str(self.key.name())
    }
}

impl FromStr for KeyCombo {
    type Err = String;

    /// `Ctrl+Shift+O` の形式を読む。修飾キーの大文字小文字と順番は問わない。
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts: Vec<&str> = s.split('+').map(str::trim).collect();
        // "Ctrl++" のようにキー自体が + のとき、split で末尾が空になる。
        if parts.len() >= 2
            && parts[parts.len() - 1].is_empty()
            && parts[parts.len() - 2].is_empty()
        {
            parts.truncate(parts.len() - 2);
            parts.push("+");
        }
        let (key_name, mods) = parts
            .split_last()
            .ok_or_else(|| format!("empty key: {s:?}"))?;
        let key = Key::from_name(key_name)
            .filter(|k| !is_modifier(*k))
            .ok_or_else(|| format!("unknown key {key_name:?} in {s:?}"))?;
        let mut combo = KeyCombo::plain(key);
        for m in mods {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => combo.ctrl = true,
                "shift" => combo.shift = true,
                "alt" => combo.alt = true,
                _ => return Err(format!("unknown modifier {m:?} in {s:?}")),
            }
        }
        Ok(combo)
    }
}

/// 修飾キーそのもの。egui は修飾キー単独の押下もキーイベントとして届けるが、
/// それだけでは割り当ての対象にしない（Ctrl を押した瞬間に登録されてしまうため）。
pub fn is_modifier(key: Key) -> bool {
    use Key::*;
    matches!(
        key,
        ShiftLeft
            | ShiftRight
            | ControlLeft
            | ControlRight
            | AltLeft
            | AltRight
            | SuperLeft
            | SuperRight
    )
}

/// 割り当てを断った理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// そのキーは別の操作に割り当て済み。
    UsedBy(Action),
    /// 同じ操作にもう割り当ててある。
    AlreadyAssigned,
}

/// 操作ごとのキーの一覧。設定ファイルにない操作は既定のキーになる。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(into = "BTreeMap<String, Vec<String>>")]
pub struct Keymap {
    bindings: BTreeMap<Action, Vec<KeyCombo>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            bindings: Action::ALL.iter().map(|a| (*a, a.default_keys())).collect(),
        }
    }
}

/// 設定ファイルの 1 操作分（キー名の配列）を読む。1 つでも読めなければ全体を不正とする。
/// 読めたキーだけを残さないのは、残りの割り当てが利用者の意図と違う形で効いてしまうため。
fn parse_keys(value: &toml::Value) -> Result<Vec<KeyCombo>, String> {
    let items = value
        .as_array()
        .ok_or_else(|| format!("expected a list of keys, found {value}"))?;
    let mut combos = Vec::new();
    for item in items {
        let text = item
            .as_str()
            .ok_or_else(|| format!("expected a key name, found {item}"))?;
        let combo = text.parse::<KeyCombo>()?;
        if !combos.contains(&combo) {
            combos.push(combo);
        }
    }
    Ok(combos)
}

impl Keymap {
    /// 設定ファイルの `[keys]` を読む。書かれていない操作は既定のキー。
    /// 読めない操作と、他の操作とキーが重なった操作は既定のキーに戻し、その旨を返す。
    pub fn from_table(table: &toml::Table) -> (Self, Vec<String>) {
        let mut map = Keymap::default();
        let mut problems = Vec::new();
        for (id, value) in table {
            let Some(action) = Action::from_id(id) else {
                problems.push(format!("keys.{id}: unknown action (ignored)"));
                continue;
            };
            match parse_keys(value) {
                Ok(keys) if action.legacy_default_keys().as_ref() == Some(&keys) => {
                    map.bindings.insert(action, action.default_keys());
                }
                Ok(keys) => {
                    map.bindings.insert(action, keys);
                }
                Err(e) => problems.push(format!("keys.{id}: {e} (using default keys)")),
            }
        }
        // 既定に戻した操作のキーが、別の操作に書かれたキーと新たに重なることがあるので、
        // 重なりがなくなるまで繰り返す。既定のキー同士は重ならないので必ず終わる。
        while let Some((combo, actions)) = map.duplicates().into_iter().next() {
            let custom: Vec<Action> = actions
                .iter()
                .copied()
                .filter(|a| !map.is_default(*a))
                .collect();
            if custom.is_empty() {
                break;
            }
            for a in &custom {
                map.bindings.insert(*a, a.default_keys());
            }
            let ids: Vec<&str> = custom.iter().map(|a| a.id()).collect();
            problems.push(format!(
                "keys: {combo} is assigned to several actions; using default keys for {}",
                ids.join(", ")
            ));
        }
        (map, problems)
    }
}

impl From<Keymap> for BTreeMap<String, Vec<String>> {
    fn from(map: Keymap) -> Self {
        map.bindings
            .into_iter()
            .map(|(a, keys)| {
                (
                    a.id().to_owned(),
                    keys.iter().map(KeyCombo::to_string).collect(),
                )
            })
            .collect()
    }
}

impl Keymap {
    pub fn keys(&self, action: Action) -> &[KeyCombo] {
        self.bindings.get(&action).map_or(&[], Vec::as_slice)
    }

    /// `combo` が割り当てられている操作。ファイルを手で編集して重複していたら、
    /// [`Action::ALL`] の順で先のものを返す。
    pub fn action_for(&self, combo: KeyCombo) -> Option<Action> {
        Action::ALL
            .into_iter()
            .find(|a| self.keys(*a).contains(&combo))
    }

    /// `combo` を `action` に足す。別の操作に割り当て済みのキーは受け付けない。
    pub fn assign(&mut self, action: Action, combo: KeyCombo) -> Result<(), Refusal> {
        match self.action_for(combo) {
            Some(a) if a == action => Err(Refusal::AlreadyAssigned),
            Some(a) => Err(Refusal::UsedBy(a)),
            None => {
                self.bindings.entry(action).or_default().push(combo);
                Ok(())
            }
        }
    }

    pub fn unassign(&mut self, action: Action, combo: KeyCombo) {
        if let Some(keys) = self.bindings.get_mut(&action) {
            keys.retain(|k| *k != combo);
        }
    }

    pub fn is_default(&self, action: Action) -> bool {
        self.keys(action) == action.default_keys()
    }

    /// `action` を既定のキーに戻す。既定のキーが今は別の操作に割り当てられていれば、
    /// 重複を作らないようにそのキーは外す。
    pub fn reset(&mut self, action: Action) {
        let keys = action
            .default_keys()
            .into_iter()
            .filter(|k| self.action_for(*k).is_none_or(|a| a == action))
            .collect();
        self.bindings.insert(action, keys);
    }

    /// 2 つ以上の操作に割り当てられているキー。設定ファイルを手で編集したときに起こる。
    pub fn duplicates(&self) -> Vec<(KeyCombo, Vec<Action>)> {
        let mut seen: Vec<(KeyCombo, Vec<Action>)> = Vec::new();
        for a in Action::ALL {
            for k in self.keys(a) {
                match seen.iter_mut().find(|(c, _)| c == k) {
                    Some((_, actions)) => actions.push(a),
                    None => seen.push((*k, vec![a])),
                }
            }
        }
        seen.retain(|(_, actions)| actions.len() > 1);
        seen
    }

    /// このフレームの入力イベントから、実行する操作を順に取り出す。
    pub fn actions_in(&self, events: &[Event]) -> Vec<Action> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Key {
                    key,
                    pressed: true,
                    repeat,
                    modifiers,
                    ..
                } => self
                    .action_for(KeyCombo::from_event(*key, *modifiers))
                    .filter(|a| !repeat || a.repeats()),
                _ => None,
            })
            .collect()
    }
}

/// OS の種類。予約キーの一覧を切り替えるのに使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
}

impl Os {
    pub const CURRENT: Os = if cfg!(windows) {
        Os::Windows
    } else {
        Os::Linux
    };
}

/// OS やデスクトップ環境が使っているキーなら、その用途を返す。
///
/// 割り当て自体は止めない。デスクトップ環境の設定で空いていることもあり、
/// 使えるかどうかは利用者にしか判断できないため。
/// Ctrl+C / X / V などのクリップボード操作はここにない。egui がキーではなく
/// コピー・貼り付けのイベントとして届けるので、そもそも割り当てられない。
pub fn os_reserved(os: Os, combo: KeyCombo) -> Option<&'static str> {
    use Key::*;
    let k = KeyCombo::plain;
    // (ctrl, shift, alt) の組み合わせを短く書くため。
    let c = |key| k(key).with(true, false, false);
    let a = |key| k(key).with(false, false, true);
    let ca = |key| k(key).with(true, false, true);
    let cs = |key| k(key).with(true, true, false);
    let common: [(KeyCombo, &str); 7] = [
        (a(F4), "Close window"),
        (a(Tab), "Switch windows"),
        (k(Tab).with(false, true, true), "Switch windows"),
        (a(Space), "Window menu"),
        (ca(Delete), "System menu"),
        (c(A), "Select all"),
        (c(Z), "Undo"),
    ];
    let windows: [(KeyCombo, &str); 5] = [
        (cs(Escape), "Task Manager"),
        (c(Escape), "Start menu"),
        (a(Escape), "Switch windows"),
        (ca(Tab), "Switch windows"),
        (c(Y), "Redo"),
    ];
    let linux: [(KeyCombo, &str); 13] = [
        (a(F1), "Application menu"),
        (a(F2), "Run command"),
        (a(F7), "Move window"),
        (a(F8), "Resize window"),
        (a(F10), "Maximize window"),
        (ca(T), "Open terminal"),
        (ca(L), "Lock screen"),
        (ca(Backspace), "Restart display server"),
        (ca(ArrowLeft), "Switch workspace"),
        (ca(ArrowRight), "Switch workspace"),
        (ca(ArrowUp), "Switch workspace"),
        (ca(ArrowDown), "Switch workspace"),
        (cs(Z), "Redo"),
    ];
    let specific: &[(KeyCombo, &str)] = match os {
        Os::Windows => &windows,
        Os::Linux => &linux,
    };
    let vt_switch = os == Os::Linux
        && combo.ctrl
        && combo.alt
        && !combo.shift
        && matches!(
            combo.key,
            F1 | F2 | F3 | F4 | F5 | F6 | F7 | F8 | F9 | F10 | F11 | F12
        );
    if vt_switch {
        return Some("Switch virtual console");
    }
    common
        .iter()
        .chain(specific)
        .find(|(r, _)| *r == combo)
        .map(|(_, what)| *what)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(s: &str) -> KeyCombo {
        s.parse().unwrap()
    }

    fn key_event(key: Key, modifiers: Modifiers, repeat: bool) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers,
        }
    }

    #[test]
    fn combo_round_trips_through_text() {
        for s in [
            "Right",
            "Ctrl+N",
            "Ctrl+Shift+Alt+F5",
            "Ctrl+Comma",
            "Shift+Space",
            "Ctrl+1",
        ] {
            assert_eq!(combo(s).to_string(), s);
        }
    }

    #[test]
    fn combo_text_ignores_modifier_case_and_order() {
        assert_eq!(combo("shift+CTRL+o"), combo("Ctrl+Shift+O"));
        assert_eq!(combo("Control + W"), KeyCombo::ctrl(Key::W));
    }

    #[test]
    fn plus_key_can_be_written_after_a_modifier() {
        assert_eq!(combo("Ctrl++"), KeyCombo::ctrl(Key::Plus));
    }

    #[test]
    fn invalid_combo_text_is_rejected_with_reason() {
        assert!("Ctrl+NoSuchKey"
            .parse::<KeyCombo>()
            .unwrap_err()
            .contains("NoSuchKey"));
        assert!("Hyper+A".parse::<KeyCombo>().unwrap_err().contains("Hyper"));
        assert!("".parse::<KeyCombo>().is_err());
    }

    #[test]
    fn modifier_alone_is_not_a_key() {
        assert!("AltLeft".parse::<KeyCombo>().is_err());
        assert!("Ctrl+ShiftLeft".parse::<KeyCombo>().is_err());
    }

    #[test]
    fn defaults_keep_the_original_navigation_keys() {
        let map = Keymap::default();
        assert_eq!(map.action_for(combo("Right")), Some(Action::NextImage));
        assert_eq!(map.action_for(combo("Space")), Some(Action::NextImage));
        assert_eq!(map.action_for(combo("Left")), Some(Action::PreviousImage));
        assert_eq!(
            map.action_for(combo("Ctrl+Comma")),
            Some(Action::ToggleSettings)
        );
    }

    #[test]
    fn defaults_have_no_duplicates() {
        assert!(Keymap::default().duplicates().is_empty());
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let map = Keymap::default();
        assert_eq!(map.action_for(combo("Shift+Right")), None);
        assert_eq!(map.action_for(combo("N")), None);
    }

    #[test]
    fn an_action_can_have_several_keys() {
        let mut map = Keymap::default();
        map.assign(Action::NewSlot, combo("Insert")).unwrap();
        assert_eq!(
            map.keys(Action::NewSlot),
            [combo("Ctrl+N"), combo("Insert")]
        );
    }

    #[test]
    fn key_used_by_another_action_is_refused() {
        let mut map = Keymap::default();
        assert_eq!(
            map.assign(Action::NewSlot, combo("Right")),
            Err(Refusal::UsedBy(Action::NextImage))
        );
        assert_eq!(map.keys(Action::NewSlot), [combo("Ctrl+N")]);
    }

    #[test]
    fn same_key_twice_on_one_action_is_refused() {
        let mut map = Keymap::default();
        assert_eq!(
            map.assign(Action::NewSlot, combo("Ctrl+N")),
            Err(Refusal::AlreadyAssigned)
        );
    }

    #[test]
    fn unassigned_key_becomes_free_for_other_actions() {
        let mut map = Keymap::default();
        map.unassign(Action::NextImage, combo("Space"));
        assert_eq!(map.action_for(combo("Space")), None);
        map.assign(Action::NewSlot, combo("Space")).unwrap();
        assert_eq!(map.action_for(combo("Space")), Some(Action::NewSlot));
    }

    #[test]
    fn reset_restores_defaults_without_stealing_keys() {
        let mut map = Keymap::default();
        map.unassign(Action::NextImage, combo("Space"));
        map.assign(Action::NewSlot, combo("Space")).unwrap();
        map.unassign(Action::NextImage, combo("Right"));
        assert!(!map.is_default(Action::NextImage));
        map.reset(Action::NextImage);
        assert_eq!(map.keys(Action::NextImage), [combo("Right")]);
        assert_eq!(map.action_for(combo("Space")), Some(Action::NewSlot));
    }

    fn load(text: &str) -> (Keymap, Vec<String>) {
        Keymap::from_table(&text.parse::<toml::Table>().unwrap())
    }

    #[test]
    fn old_default_image_keys_become_the_new_defaults_without_warning() {
        let (map, problems) = load(
            "next_image = [\"Right\", \"PageDown\", \"Space\"]\n\
             previous_image = [\"Left\", \"PageUp\"]",
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(map.action_for(combo("PageDown")), Some(Action::NextPage));
        assert_eq!(map.action_for(combo("PageUp")), Some(Action::PreviousPage));
        assert!(map.is_default(Action::NextImage));
    }

    #[test]
    fn page_keys_page_and_arrows_step_images_by_default() {
        let map = Keymap::default();
        assert_eq!(map.action_for(combo("PageDown")), Some(Action::NextPage));
        assert_eq!(map.action_for(combo("Right")), Some(Action::NextImage));
        assert_eq!(map.action_for(combo("Ctrl+Right")), Some(Action::NextSlot));
        assert_eq!(map.action_for(combo("Enter")), Some(Action::ToggleMaximize));
    }

    #[test]
    fn file_entries_override_only_the_listed_actions() {
        let (map, problems) = load("new_slot = [\"Insert\"]");
        assert_eq!(map.keys(Action::NewSlot), [combo("Insert")]);
        assert_eq!(map.keys(Action::CloseSlot), [combo("Ctrl+W")]);
        assert!(problems.is_empty());
    }

    #[test]
    fn empty_list_in_file_leaves_action_unbound() {
        let (map, problems) = load("new_slot = []");
        assert!(map.keys(Action::NewSlot).is_empty());
        assert!(problems.is_empty());
    }

    #[test]
    fn unknown_action_is_reported_and_ignored() {
        let (map, problems) = load("fly = [\"F\"]");
        assert_eq!(map, Keymap::default());
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn action_with_any_bad_key_falls_back_to_its_defaults() {
        let (map, problems) = load("new_slot = [\"Ctrl+Nope\", \"Insert\"]");
        assert_eq!(map.keys(Action::NewSlot), [combo("Ctrl+N")]);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn action_with_wrong_value_type_falls_back_to_its_defaults() {
        for text in [
            "new_slot = \"Insert\"",
            "new_slot = [1]",
            "new_slot = { a = 1 }",
        ] {
            let (map, problems) = load(text);
            assert_eq!(map.keys(Action::NewSlot), [combo("Ctrl+N")], "{text}");
            assert_eq!(problems.len(), 1, "{text}");
        }
    }

    #[test]
    fn hand_written_duplicate_reverts_the_customized_action() {
        let (map, problems) = load("new_slot = [\"Right\"]");
        assert_eq!(map, Keymap::default());
        assert!(map.duplicates().is_empty());
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn duplicate_between_two_customized_actions_reverts_both() {
        let (map, problems) = load("new_slot = [\"F2\"]\nclose_slot = [\"F2\"]");
        assert_eq!(map, Keymap::default());
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn reverting_does_not_leave_a_new_duplicate() {
        // next_image が壊れて既定（Right など）に戻ると、Right を書いた new_slot と重なる。
        let (map, _) = load("next_image = [\"Nope\"]\nnew_slot = [\"Right\"]");
        assert!(map.duplicates().is_empty());
        assert_eq!(map, Keymap::default());
    }

    #[test]
    fn saved_form_reads_back_identically() {
        let mut map = Keymap::default();
        map.assign(Action::CloseSlot, combo("Ctrl+Shift+F4"))
            .unwrap();
        let raw: BTreeMap<String, Vec<String>> = map.clone().into();
        let table = toml::Table::try_from(raw).unwrap();
        assert_eq!(Keymap::from_table(&table), (map, vec![]));
    }

    #[test]
    fn key_events_become_actions() {
        let map = Keymap::default();
        let events = [
            key_event(Key::ArrowRight, Modifiers::NONE, false),
            key_event(Key::N, Modifiers::CTRL, false),
            key_event(Key::Q, Modifiers::NONE, false),
        ];
        assert_eq!(
            map.actions_in(&events),
            [Action::NextImage, Action::NewSlot]
        );
    }

    #[test]
    fn held_key_repeats_navigation_but_not_toggles() {
        let map = Keymap::default();
        let events = [
            key_event(Key::ArrowRight, Modifiers::NONE, true),
            key_event(Key::Num4, Modifiers::CTRL, true),
        ];
        assert_eq!(map.actions_in(&events), [Action::NextImage]);
    }

    #[test]
    fn os_window_keys_are_flagged_on_both_platforms() {
        for os in [Os::Windows, Os::Linux] {
            assert_eq!(os_reserved(os, combo("Alt+F4")), Some("Close window"));
            assert_eq!(os_reserved(os, combo("Alt+Tab")), Some("Switch windows"));
        }
    }

    #[test]
    fn platform_specific_keys_are_flagged_only_on_that_platform() {
        assert_eq!(
            os_reserved(Os::Windows, combo("Ctrl+Shift+Escape")),
            Some("Task Manager")
        );
        assert_eq!(os_reserved(Os::Linux, combo("Ctrl+Shift+Escape")), None);
        assert_eq!(
            os_reserved(Os::Linux, combo("Ctrl+Alt+T")),
            Some("Open terminal")
        );
        assert_eq!(os_reserved(Os::Windows, combo("Ctrl+Alt+T")), None);
        assert_eq!(
            os_reserved(Os::Linux, combo("Ctrl+Alt+F3")),
            Some("Switch virtual console")
        );
    }

    #[test]
    fn ordinary_keys_are_not_flagged() {
        for os in [Os::Windows, Os::Linux] {
            for s in ["Right", "Ctrl+N", "Ctrl+Comma", "F5", "Shift+Alt+F4"] {
                assert_eq!(os_reserved(os, combo(s)), None, "{s}");
            }
        }
    }

    #[test]
    fn default_keys_avoid_os_keys() {
        let map = Keymap::default();
        for os in [Os::Windows, Os::Linux] {
            for a in Action::ALL {
                for k in map.keys(a) {
                    assert_eq!(os_reserved(os, *k), None, "{k}");
                }
            }
        }
    }
}
