//! The key rebinding view: one row per action, press a key or a mouse button
//! to rebind, and a swap-or-cancel choice when the input is already bound.
//!
//! [`RebindView`] holds the state and the logic (testable without a window);
//! [`RebindView::show`] draws it with egui.

use faset_core::bindings::{Action, Assign, Bindings, Input};

use crate::i18n::{fill, text};

/// Outcome of a rebinding step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebindResult {
    /// No action is waiting for an input.
    Idle,
    /// The action now uses the input.
    Bound,
    /// Another action uses the input; the view waits for swap or cancel.
    Conflict {
        /// The action that has the input now.
        other: Action,
        /// The input the player pressed.
        input: Input,
    },
    /// The input is the reserved pause key: nothing changed.
    Reserved(Input),
    /// The two actions exchanged their inputs.
    Swapped(Action),
    /// The player cancelled; nothing changed.
    Cancelled,
}

/// State of the rebinding view.
#[derive(Debug, Default)]
pub struct RebindView {
    /// The action waiting for an input.
    pub listening: Option<Action>,
    /// A conflict waiting for the player: action, wanted input, other action.
    pub pending: Option<(Action, Input, Action)>,
    /// What happened last, for the message line.
    pub last: Option<RebindResult>,
    modifiers: (bool, bool, bool),
}

impl RebindView {
    /// Starts waiting for an input for `action`.
    pub fn start(&mut self, action: Action) {
        self.listening = Some(action);
        self.pending = None;
        self.last = None;
    }

    /// Gives the input the player pressed. Does nothing unless an action is
    /// listening.
    pub fn capture(&mut self, bindings: &mut Bindings, input: Input) -> RebindResult {
        let Some(action) = self.listening else {
            return RebindResult::Idle;
        };
        let result = match bindings.assign(action, input.clone()) {
            Assign::Bound => RebindResult::Bound,
            Assign::Reserved => RebindResult::Reserved(input),
            Assign::Conflict(other) => {
                self.pending = Some((action, input.clone(), other));
                RebindResult::Conflict { other, input }
            }
        };
        // A conflict keeps the choice open; a reserved key keeps listening.
        if !matches!(result, RebindResult::Reserved(_)) {
            self.listening = None;
        }
        self.last = Some(result.clone());
        result
    }

    /// Answers a pending conflict: swap the two inputs or cancel.
    pub fn resolve(&mut self, bindings: &mut Bindings, swap: bool) -> RebindResult {
        let Some((action, input, other)) = self.pending.take() else {
            return RebindResult::Idle;
        };
        let result = if swap {
            bindings.swap(action, input, other);
            RebindResult::Swapped(other)
        } else {
            RebindResult::Cancelled
        };
        self.last = Some(result.clone());
        result
    }

    /// Gives up waiting for an input.
    pub fn cancel(&mut self) {
        self.listening = None;
        self.pending = None;
        self.last = Some(RebindResult::Cancelled);
    }

    fn message(&self) -> Option<String> {
        let name = |a: Action| text(a.label_key()).to_owned();
        match self.pending.as_ref() {
            Some((_, input, other)) => Some(fill(
                text("controls.conflict"),
                &[("input", &input.label()), ("action", &name(*other))],
            )),
            None => match self.last.as_ref()? {
                RebindResult::Reserved(i) => {
                    Some(fill(text("controls.reserved"), &[("input", &i.label())]))
                }
                RebindResult::Swapped(a) => {
                    Some(fill(text("controls.swapped"), &[("action", &name(*a))]))
                }
                _ => None,
            },
        }
    }

    /// Draws the view and applies the player's choices to `bindings`.
    /// Returns true when the bindings changed.
    pub fn show(&mut self, ui: &mut egui::Ui, bindings: &mut Bindings) -> bool {
        let before = bindings.clone();
        if self.listening.is_some() {
            let (events, mods) = ui.input(|i| (i.events.clone(), i.modifiers));
            if events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key {
                        key: egui::Key::Escape,
                        pressed: true,
                        ..
                    }
                )
            }) {
                self.cancel();
            } else {
                for input in events.iter().filter_map(input_of_event) {
                    self.capture(bindings, input);
                    if self.listening.is_none() {
                        break;
                    }
                }
                if self.listening.is_some() {
                    // Modifier keys arrive as modifier state, not key events.
                    let now = (mods.ctrl, mods.shift, mods.alt);
                    let (c, s, a) = self.modifiers;
                    let pressed = if now.0 && !c {
                        Some("ControlLeft")
                    } else if now.1 && !s {
                        Some("ShiftLeft")
                    } else if now.2 && !a {
                        Some("AltLeft")
                    } else {
                        None
                    };
                    if let Some(name) = pressed {
                        self.capture(bindings, Input::key(name));
                    }
                }
            }
            self.modifiers = (mods.ctrl, mods.shift, mods.alt);
        } else {
            self.modifiers = ui.input(|i| (i.modifiers.ctrl, i.modifiers.shift, i.modifiers.alt));
        }

        ui.label(text("controls.bindings"));
        for action in Action::ALL {
            ui.horizontal(|ui| {
                ui.label(text(action.label_key()));
                let shown = if self.listening == Some(action) {
                    text("controls.press_key").to_owned()
                } else {
                    bindings
                        .get(action)
                        .map_or_else(|| text("controls.unbound").to_owned(), Input::label)
                };
                let enabled = self.pending.is_none();
                if ui.add_enabled(enabled, egui::Button::new(shown)).clicked() {
                    self.start(action);
                }
            });
        }
        if let Some(message) = self.message() {
            ui.label(message);
        }
        if self.pending.is_some() {
            ui.horizontal(|ui| {
                if ui.button(text("controls.swap")).clicked() {
                    self.resolve(bindings, true);
                }
                if ui.button(text("controls.cancel")).clicked() {
                    self.resolve(bindings, false);
                }
            });
        }
        if ui.button(text("controls.reset")).clicked() {
            *bindings = Bindings::default();
            self.listening = None;
            self.pending = None;
            self.last = None;
        }
        *bindings != before
    }
}

/// The input of a key or mouse button press event, named like the windowing
/// layer names physical keys (`KeyW`, `Digit1`, `Space`).
pub fn input_of_event(event: &egui::Event) -> Option<Input> {
    match event {
        egui::Event::Key {
            key, pressed: true, ..
        } => {
            let name = format!("{key:?}");
            let physical = if name.len() == 1 && name.as_bytes()[0].is_ascii_uppercase() {
                format!("Key{name}")
            } else if let Some(digit) = name.strip_prefix("Num").filter(|d| d.len() == 1) {
                format!("Digit{digit}")
            } else {
                name
            };
            physical.parse().ok()
        }
        egui::Event::PointerButton {
            button,
            pressed: true,
            ..
        } => Some(Input::Mouse(match button {
            egui::PointerButton::Primary => 0,
            egui::PointerButton::Secondary => 1,
            egui::PointerButton::Middle => 2,
            egui::PointerButton::Extra1 => 3,
            egui::PointerButton::Extra2 => 4,
        })),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_input_binds() {
        let mut b = Bindings::default();
        let mut v = RebindView::default();
        v.start(Action::Jump);
        assert_eq!(v.capture(&mut b, Input::key("KeyZ")), RebindResult::Bound);
        assert_eq!(b.get(Action::Jump), Some(&Input::key("KeyZ")));
        assert_eq!(v.listening, None);
        // Not listening: nothing happens.
        assert_eq!(v.capture(&mut b, Input::key("KeyX")), RebindResult::Idle);
    }

    #[test]
    fn bound_input_reports_the_conflict_then_swaps() {
        let mut b = Bindings::default();
        let mut v = RebindView::default();
        v.start(Action::Jump);
        let r = v.capture(&mut b, Input::key("KeyW"));
        assert_eq!(
            r,
            RebindResult::Conflict {
                other: Action::Forward,
                input: Input::key("KeyW")
            }
        );
        // Nothing changed yet.
        assert_eq!(b.get(Action::Jump), Some(&Input::key("Space")));
        assert_eq!(
            v.resolve(&mut b, true),
            RebindResult::Swapped(Action::Forward)
        );
        assert_eq!(b.get(Action::Jump), Some(&Input::key("KeyW")));
        assert_eq!(b.get(Action::Forward), Some(&Input::key("Space")));
    }

    #[test]
    fn conflict_can_be_cancelled() {
        let mut b = Bindings::default();
        let before = b.clone();
        let mut v = RebindView::default();
        v.start(Action::Jump);
        v.capture(&mut b, Input::key("KeyW"));
        assert_eq!(v.resolve(&mut b, false), RebindResult::Cancelled);
        assert_eq!(b, before);
        assert_eq!(v.resolve(&mut b, true), RebindResult::Idle);
    }

    #[test]
    fn reserved_key_keeps_listening() {
        let mut b = Bindings::default();
        let mut v = RebindView::default();
        v.start(Action::Jump);
        assert!(matches!(
            v.capture(&mut b, Input::key("Escape")),
            RebindResult::Reserved(_)
        ));
        assert_eq!(v.listening, Some(Action::Jump));
    }

    #[test]
    fn events_map_to_physical_names() {
        let key = |k| egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        assert_eq!(input_of_event(&key(egui::Key::W)), Some(Input::key("KeyW")));
        assert_eq!(
            input_of_event(&key(egui::Key::Num1)),
            Some(Input::key("Digit1"))
        );
        assert_eq!(
            input_of_event(&key(egui::Key::Space)),
            Some(Input::key("Space"))
        );
    }

    #[test]
    fn view_runs_headless() {
        let ctx = egui::Context::default();
        let mut b = Bindings::default();
        let mut v = RebindView::default();
        v.start(Action::Fire);
        ctx.begin_pass(egui::RawInput::default());
        egui::Window::new("rebinding").show(&ctx, |ui| {
            assert!(!v.show(ui, &mut b));
        });
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
    }
}
