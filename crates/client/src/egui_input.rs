//! The winit to egui input adapter (lot 6.6).
//!
//! [`EguiInput`] turns window events into the [`egui::Event`]s a
//! [`faset_ui::Ui`] frame takes: pointer motion in points (physical pixels
//! over the pixels-per-point of the UI, scale factor and interface scale
//! included), buttons, wheel, keys with the modifiers held, typed text and
//! input-method commits. It also names physical keys the way the bindings do
//! ([`physical_name`]) so the rebinding screen can read any key, modifiers
//! included, and keeps the cursor position the game needs.

use faset_ui::egui::{self, Event, PointerButton};
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

/// The name the bindings give a physical key (`KeyW`, `Digit1`, `Space`,
/// `ShiftLeft`): the name of winit's [`KeyCode`] variant.
#[must_use]
pub fn physical_name(code: KeyCode) -> String {
    format!("{code:?}")
}

/// The egui key of a physical key, for the keys egui knows.
#[must_use]
pub fn egui_key(code: KeyCode) -> Option<egui::Key> {
    use egui::Key as K;
    Some(match code {
        KeyCode::KeyA => K::A,
        KeyCode::KeyB => K::B,
        KeyCode::KeyC => K::C,
        KeyCode::KeyD => K::D,
        KeyCode::KeyE => K::E,
        KeyCode::KeyF => K::F,
        KeyCode::KeyG => K::G,
        KeyCode::KeyH => K::H,
        KeyCode::KeyI => K::I,
        KeyCode::KeyJ => K::J,
        KeyCode::KeyK => K::K,
        KeyCode::KeyL => K::L,
        KeyCode::KeyM => K::M,
        KeyCode::KeyN => K::N,
        KeyCode::KeyO => K::O,
        KeyCode::KeyP => K::P,
        KeyCode::KeyQ => K::Q,
        KeyCode::KeyR => K::R,
        KeyCode::KeyS => K::S,
        KeyCode::KeyT => K::T,
        KeyCode::KeyU => K::U,
        KeyCode::KeyV => K::V,
        KeyCode::KeyW => K::W,
        KeyCode::KeyX => K::X,
        KeyCode::KeyY => K::Y,
        KeyCode::KeyZ => K::Z,
        KeyCode::Digit0 | KeyCode::Numpad0 => K::Num0,
        KeyCode::Digit1 | KeyCode::Numpad1 => K::Num1,
        KeyCode::Digit2 | KeyCode::Numpad2 => K::Num2,
        KeyCode::Digit3 | KeyCode::Numpad3 => K::Num3,
        KeyCode::Digit4 | KeyCode::Numpad4 => K::Num4,
        KeyCode::Digit5 | KeyCode::Numpad5 => K::Num5,
        KeyCode::Digit6 | KeyCode::Numpad6 => K::Num6,
        KeyCode::Digit7 | KeyCode::Numpad7 => K::Num7,
        KeyCode::Digit8 | KeyCode::Numpad8 => K::Num8,
        KeyCode::Digit9 | KeyCode::Numpad9 => K::Num9,
        KeyCode::Space => K::Space,
        KeyCode::Enter | KeyCode::NumpadEnter => K::Enter,
        KeyCode::Tab => K::Tab,
        KeyCode::Backspace => K::Backspace,
        KeyCode::Delete => K::Delete,
        KeyCode::Insert => K::Insert,
        KeyCode::Escape => K::Escape,
        KeyCode::Home => K::Home,
        KeyCode::End => K::End,
        KeyCode::PageUp => K::PageUp,
        KeyCode::PageDown => K::PageDown,
        KeyCode::ArrowLeft => K::ArrowLeft,
        KeyCode::ArrowRight => K::ArrowRight,
        KeyCode::ArrowUp => K::ArrowUp,
        KeyCode::ArrowDown => K::ArrowDown,
        KeyCode::Minus | KeyCode::NumpadSubtract => K::Minus,
        KeyCode::Equal => K::Equals,
        KeyCode::NumpadAdd => K::Plus,
        KeyCode::Comma => K::Comma,
        KeyCode::Period | KeyCode::NumpadDecimal => K::Period,
        KeyCode::Semicolon => K::Semicolon,
        KeyCode::Slash | KeyCode::NumpadDivide => K::Slash,
        KeyCode::Backslash => K::Backslash,
        KeyCode::Quote => K::Quote,
        KeyCode::Backquote => K::Backtick,
        KeyCode::BracketLeft => K::OpenBracket,
        KeyCode::BracketRight => K::CloseBracket,
        KeyCode::F1 => K::F1,
        KeyCode::F2 => K::F2,
        KeyCode::F3 => K::F3,
        KeyCode::F4 => K::F4,
        KeyCode::F5 => K::F5,
        KeyCode::F6 => K::F6,
        KeyCode::F7 => K::F7,
        KeyCode::F8 => K::F8,
        KeyCode::F9 => K::F9,
        KeyCode::F10 => K::F10,
        KeyCode::F11 => K::F11,
        KeyCode::F12 => K::F12,
        _ => return None,
    })
}

/// The binding name of a mouse button: `Mouse0` left, `Mouse1` right,
/// `Mouse2` middle, then the side buttons.
#[must_use]
pub fn mouse_index(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Right => Some(1),
        MouseButton::Middle => Some(2),
        MouseButton::Back => Some(3),
        MouseButton::Forward => Some(4),
        MouseButton::Other(_) => None,
    }
}

fn pointer_button(button: MouseButton) -> Option<PointerButton> {
    Some(match button {
        MouseButton::Left => PointerButton::Primary,
        MouseButton::Right => PointerButton::Secondary,
        MouseButton::Middle => PointerButton::Middle,
        MouseButton::Back => PointerButton::Extra1,
        MouseButton::Forward => PointerButton::Extra2,
        MouseButton::Other(_) => return None,
    })
}

/// A game key press the executable must also see: the physical key and its
/// state, or a mouse button.
#[derive(Debug, Clone, PartialEq)]
pub enum RawInput {
    /// A keyboard key, by binding name.
    Key { name: String, pressed: bool },
    /// A mouse button, by index.
    Mouse { index: u8, pressed: bool },
    /// The wheel turned (lines, positive away from the player).
    Wheel(f32),
}

/// Accumulates the egui events of a frame.
#[derive(Debug, Default)]
pub struct EguiInput {
    events: Vec<Event>,
    modifiers: egui::Modifiers,
    /// Cursor position in physical pixels.
    pointer_px: (f32, f32),
    raw: Vec<RawInput>,
}

fn modifiers_of(state: ModifiersState) -> egui::Modifiers {
    egui::Modifiers {
        alt: state.alt_key(),
        ctrl: state.control_key(),
        shift: state.shift_key(),
        mac_cmd: false,
        command: state.control_key(),
    }
}

impl EguiInput {
    /// An empty adapter.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The cursor position in physical pixels.
    #[must_use]
    pub fn pointer_px(&self) -> (f32, f32) {
        self.pointer_px
    }

    /// The modifiers held.
    #[must_use]
    pub fn modifiers(&self) -> egui::Modifiers {
        self.modifiers
    }

    /// The events of the frame, for `FrameInput::events`.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// The raw key, button and wheel events of the frame, for the game.
    pub fn take_raw(&mut self) -> Vec<RawInput> {
        std::mem::take(&mut self.raw)
    }

    /// Drops what was gathered (a match that grabs the mouse does not feed
    /// the menus).
    pub fn clear_events(&mut self) {
        self.events.clear();
    }

    fn point(&self, ppp: f32) -> egui::Pos2 {
        egui::pos2(self.pointer_px.0 / ppp, self.pointer_px.1 / ppp)
    }

    /// Takes one window event. `ppp` is the number of physical pixels per
    /// egui point.
    pub fn on_window_event(&mut self, event: &WindowEvent, ppp: f32) {
        let ppp = ppp.max(0.01);
        match event {
            WindowEvent::ModifiersChanged(m) => self.modifiers = modifiers_of(m.state()),
            WindowEvent::Focused(false) => {
                self.modifiers = egui::Modifiers::NONE;
                self.events.push(Event::WindowFocused(false));
            }
            WindowEvent::Focused(true) => self.events.push(Event::WindowFocused(true)),
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer_px = (position.x as f32, position.y as f32);
                self.events.push(Event::PointerMoved(self.point(ppp)));
            }
            WindowEvent::CursorLeft { .. } => self.events.push(Event::PointerGone),
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = *state == ElementState::Pressed;
                if let Some(index) = mouse_index(*button) {
                    self.raw.push(RawInput::Mouse { index, pressed });
                }
                if let Some(button) = pointer_button(*button) {
                    self.events.push(Event::PointerButton {
                        pos: self.point(ppp),
                        button,
                        pressed,
                        modifiers: self.modifiers,
                    });
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (unit, delta, lines) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        (egui::MouseWheelUnit::Line, egui::vec2(*x, *y), *y)
                    }
                    MouseScrollDelta::PixelDelta(p) => (
                        egui::MouseWheelUnit::Point,
                        egui::vec2(p.x as f32 / ppp, p.y as f32 / ppp),
                        p.y as f32 / 40.0,
                    ),
                };
                self.raw.push(RawInput::Wheel(lines));
                self.events.push(Event::MouseWheel {
                    unit,
                    delta,
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Wheel,
                    modifiers: self.modifiers,
                });
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                let code = match event.physical_key {
                    PhysicalKey::Code(code) => Some(code),
                    PhysicalKey::Unidentified(_) => None,
                };
                if let Some(code) = code {
                    if !event.repeat {
                        self.raw.push(RawInput::Key {
                            name: physical_name(code),
                            pressed,
                        });
                    }
                }
                let key = code.and_then(egui_key).or(match &event.logical_key {
                    Key::Named(NamedKey::Escape) => Some(egui::Key::Escape),
                    _ => None,
                });
                if let Some(key) = key {
                    self.events.push(Event::Key {
                        key,
                        physical_key: Some(key),
                        pressed,
                        repeat: event.repeat,
                        modifiers: self.modifiers,
                    });
                }
                let typing = !self.modifiers.ctrl && !self.modifiers.alt;
                if pressed && typing {
                    if let Some(text) = event
                        .text
                        .as_ref()
                        .filter(|t| !t.is_empty() && !t.chars().any(char::is_control))
                    {
                        self.events.push(Event::Text(text.to_string()));
                    }
                }
            }
            WindowEvent::Ime(Ime::Commit(text)) if !text.is_empty() => {
                self.events.push(Event::Text(text.clone()));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_names_match_the_bindings() {
        assert_eq!(physical_name(KeyCode::KeyW), "KeyW");
        assert_eq!(physical_name(KeyCode::Digit1), "Digit1");
        assert_eq!(physical_name(KeyCode::ShiftLeft), "ShiftLeft");
        assert_eq!(physical_name(KeyCode::Space), "Space");
        assert!(
            faset_core::bindings::Input::key(&physical_name(KeyCode::ControlLeft))
                .to_string()
                .eq("ControlLeft")
        );
    }

    #[test]
    fn letters_digits_and_navigation_reach_egui() {
        assert_eq!(egui_key(KeyCode::KeyA), Some(egui::Key::A));
        assert_eq!(egui_key(KeyCode::Digit7), Some(egui::Key::Num7));
        assert_eq!(egui_key(KeyCode::Numpad7), Some(egui::Key::Num7));
        assert_eq!(egui_key(KeyCode::Escape), Some(egui::Key::Escape));
        assert_eq!(egui_key(KeyCode::F3), Some(egui::Key::F3));
        assert_eq!(egui_key(KeyCode::ArrowDown), Some(egui::Key::ArrowDown));
        assert_eq!(egui_key(KeyCode::ShiftLeft), None);
    }

    #[test]
    fn mouse_buttons_have_binding_indexes() {
        assert_eq!(mouse_index(MouseButton::Left), Some(0));
        assert_eq!(mouse_index(MouseButton::Right), Some(1));
        assert_eq!(mouse_index(MouseButton::Back), Some(3));
        assert_eq!(mouse_index(MouseButton::Other(9)), None);
    }

    #[test]
    fn modifiers_map_command_to_control() {
        let m = modifiers_of(ModifiersState::CONTROL | ModifiersState::SHIFT);
        assert!(m.ctrl && m.shift && m.command && !m.alt);
    }

    #[test]
    fn focus_loss_clears_the_modifiers_and_tells_egui() {
        let mut input = EguiInput::new();
        input.modifiers.shift = true;
        input.on_window_event(&WindowEvent::Focused(false), 1.0);
        assert_eq!(input.modifiers(), egui::Modifiers::NONE);
        assert_eq!(input.take_events(), vec![Event::WindowFocused(false)]);
    }

    #[test]
    fn ime_commits_become_text() {
        let mut input = EguiInput::new();
        input.on_window_event(&WindowEvent::Ime(Ime::Commit("é".to_owned())), 1.0);
        assert_eq!(input.take_events(), vec![Event::Text("é".to_owned())]);
    }
}
