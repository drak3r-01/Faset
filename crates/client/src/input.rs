//! Player input (lot 6.6): the keys and mouse of the window become the
//! [`InputCommand`] of each simulation tick.
//!
//! [`GameInput`] holds the physical state (keys held, look angles, toggles)
//! and knows nothing of the windowing layer: the executable names each key
//! like the bindings do (`KeyW`, `Space`, `Mouse0`, see
//! [`faset_core::bindings`]) and feeds [`GameInput::press`] and
//! [`GameInput::mouse_motion`]. The command is built from the user's
//! [`Bindings`] and [`ControlSettings`], so a rebinding or a sensitivity
//! change takes effect at once.
//!
//! Not sent: `Sprint` and `Interact` have no bit in the protocol yet.

use std::collections::HashSet;

use faset_core::bindings::{Action, Bindings, Input};
use faset_core::settings::ControlSettings;
use faset_core::Tick;
use faset_protocol::{
    quant::{quantize_pitch_command, quantize_yaw},
    InputCommand, BUTTON_CROUCH, BUTTON_FIRE, BUTTON_JUMP, BUTTON_RELOAD, BUTTON_SIGHT,
};

use crate::camera::Look;
use crate::feel::degrees_per_count;

/// Furthest the view looks up or down, radians (just short of vertical).
const PITCH_LIMIT: f32 = 89.0 * std::f32::consts::PI / 180.0;

/// The player's physical input.
#[derive(Debug, Clone, PartialEq)]
pub struct GameInput {
    held: HashSet<Input>,
    yaw: f32,
    pitch: f32,
    /// Weapon slot in hand: 0 main, 1 secondary.
    slot: u8,
    crouch_on: bool,
    aim_on: bool,
    /// A jump press not yet sent (jump that does not repeat when held).
    jump_pending: bool,
    /// A reload press not yet sent.
    reload_pending: bool,
}

impl Default for GameInput {
    fn default() -> Self {
        Self::new()
    }
}

impl GameInput {
    /// No key held, looking along -Z with the main weapon in hand.
    #[must_use]
    pub fn new() -> Self {
        Self {
            held: HashSet::new(),
            yaw: 0.0,
            pitch: 0.0,
            slot: 0,
            crouch_on: false,
            aim_on: false,
            jump_pending: false,
            reload_pending: false,
        }
    }

    /// Releases everything (focus lost, pause menu opened).
    pub fn release_all(&mut self) {
        self.held.clear();
        self.crouch_on = false;
        self.aim_on = false;
        self.jump_pending = false;
        self.reload_pending = false;
    }

    /// Looks along `yaw` and `pitch` (radians, positive yaw turns left).
    pub fn set_look(&mut self, yaw: f32, pitch: f32) {
        self.yaw = yaw.rem_euclid(std::f32::consts::TAU);
        self.pitch = pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Where the player looks.
    #[must_use]
    pub fn look(&self) -> Look {
        Look {
            yaw: self.yaw,
            pitch: self.pitch,
        }
    }

    /// Weapon slot in hand.
    #[must_use]
    pub fn slot(&self) -> u8 {
        self.slot
    }

    fn is_held(&self, bindings: &Bindings, action: Action) -> bool {
        bindings.get(action).is_some_and(|i| self.held.contains(i))
    }

    /// A key or mouse button went down or up.
    pub fn press(
        &mut self,
        input: Input,
        pressed: bool,
        bindings: &Bindings,
        controls: &ControlSettings,
    ) {
        if !pressed {
            self.held.remove(&input);
            return;
        }
        if !self.held.insert(input.clone()) {
            return; // key repeat
        }
        match bindings.action_of(&input) {
            Some(Action::Jump) => self.jump_pending = true,
            Some(Action::Reload) => self.reload_pending = true,
            Some(Action::Crouch) if controls.crouch_toggle => self.crouch_on = !self.crouch_on,
            Some(Action::Aim) if controls.aim_toggle => self.aim_on = !self.aim_on,
            Some(Action::Slot1) => self.slot = 0,
            Some(Action::Slot2) => self.slot = 1,
            Some(Action::NextWeapon | Action::PreviousWeapon) => self.slot = 1 - self.slot,
            _ => {}
        }
    }

    /// The wheel turned by `lines` (positive: away from the player): the
    /// other weapon.
    pub fn wheel(&mut self, lines: f32) {
        if lines.abs() > f32::EPSILON {
            self.slot = 1 - self.slot;
        }
    }

    /// Whether the player looks down the sights.
    #[must_use]
    pub fn aiming(&self, bindings: &Bindings, controls: &ControlSettings) -> bool {
        if controls.aim_toggle {
            self.aim_on
        } else {
            self.is_held(bindings, Action::Aim)
        }
    }

    /// Whether the scoreboard key is held.
    #[must_use]
    pub fn scoreboard(&self, bindings: &Bindings) -> bool {
        self.is_held(bindings, Action::Scoreboard)
    }

    /// A raw mouse movement (`dx` right, `dy` down, in counts) turns the
    /// view: moving right turns right, moving up looks up (unless inverted),
    /// scaled by the sensitivity (and the aim sensitivity while aiming).
    pub fn mouse_motion(
        &mut self,
        dx: f64,
        dy: f64,
        bindings: &Bindings,
        controls: &ControlSettings,
    ) {
        let aim = if self.aiming(bindings, controls) {
            controls.aim_sensitivity
        } else {
            1.0
        };
        let step = degrees_per_count(controls.sensitivity * aim).to_radians();
        let sign = if controls.invert_y { -1.0 } else { 1.0 };
        self.set_look(
            self.yaw - dx as f32 * step,
            self.pitch - dy as f32 * step * sign,
        );
    }

    /// The command of `tick`. A jump or reload press is sent once.
    pub fn command(
        &mut self,
        tick: Tick,
        bindings: &Bindings,
        controls: &ControlSettings,
    ) -> InputCommand {
        let axis = |input: &Self, plus: Action, minus: Action| -> i8 {
            i8::from(input.is_held(bindings, plus)) * 127
                - i8::from(input.is_held(bindings, minus)) * 127
        };
        let (move_x, move_y) = (
            axis(self, Action::Right, Action::Left),
            axis(self, Action::Forward, Action::Backward),
        );
        let mut buttons = 0;
        let jump = if controls.hold_jump {
            self.is_held(bindings, Action::Jump)
        } else {
            self.jump_pending
        };
        if jump {
            buttons |= BUTTON_JUMP;
        }
        let crouch = if controls.crouch_toggle {
            self.crouch_on
        } else {
            self.is_held(bindings, Action::Crouch)
        };
        if crouch {
            buttons |= BUTTON_CROUCH;
        }
        if self.is_held(bindings, Action::Fire) {
            buttons |= BUTTON_FIRE;
        }
        if self.aiming(bindings, controls) {
            buttons |= BUTTON_SIGHT;
        }
        if self.reload_pending {
            buttons |= BUTTON_RELOAD;
        }
        self.jump_pending = false;
        self.reload_pending = false;
        InputCommand {
            tick,
            buttons,
            move_x,
            move_y,
            yaw: quantize_yaw(self.yaw),
            pitch: quantize_pitch_command(self.pitch),
            weapon_slot: self.slot,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_protocol::quant::dequantize_pitch_command;

    fn setup() -> (GameInput, Bindings, ControlSettings) {
        (
            GameInput::new(),
            Bindings::default(),
            ControlSettings::default(),
        )
    }

    fn press(i: &mut GameInput, b: &Bindings, c: &ControlSettings, key: &str, down: bool) {
        i.press(Input::key(key), down, b, c);
    }

    #[test]
    fn movement_keys_make_the_axes_and_cancel() {
        let (mut i, b, c) = setup();
        press(&mut i, &b, &c, "KeyW", true);
        press(&mut i, &b, &c, "KeyD", true);
        let cmd = i.command(Tick(1), &b, &c);
        assert_eq!((cmd.move_x, cmd.move_y), (127, 127));
        press(&mut i, &b, &c, "KeyS", true);
        assert_eq!(i.command(Tick(2), &b, &c).move_y, 0);
        press(&mut i, &b, &c, "KeyW", false);
        press(&mut i, &b, &c, "KeyS", false);
        press(&mut i, &b, &c, "KeyD", false);
        let cmd = i.command(Tick(3), &b, &c);
        assert_eq!((cmd.move_x, cmd.move_y), (0, 0));
    }

    #[test]
    fn rebinding_moves_the_action() {
        let (mut i, mut b, c) = setup();
        b.assign(Action::Forward, Input::key("ArrowUp"));
        press(&mut i, &b, &c, "KeyW", true);
        assert_eq!(i.command(Tick(1), &b, &c).move_y, 0);
        press(&mut i, &b, &c, "ArrowUp", true);
        assert_eq!(i.command(Tick(2), &b, &c).move_y, 127);
    }

    #[test]
    fn fire_and_sight_follow_the_buttons() {
        let (mut i, b, c) = setup();
        i.press(Input::Mouse(0), true, &b, &c);
        i.press(Input::Mouse(1), true, &b, &c);
        let cmd = i.command(Tick(1), &b, &c);
        assert_ne!(cmd.buttons & BUTTON_FIRE, 0);
        assert_ne!(cmd.buttons & BUTTON_SIGHT, 0);
        i.press(Input::Mouse(0), false, &b, &c);
        assert_eq!(i.command(Tick(2), &b, &c).buttons & BUTTON_FIRE, 0);
    }

    #[test]
    fn toggles_stay_on_until_pressed_again() {
        let (mut i, b, mut c) = setup();
        c.crouch_toggle = true;
        press(&mut i, &b, &c, "ControlLeft", true);
        press(&mut i, &b, &c, "ControlLeft", false);
        assert_ne!(i.command(Tick(1), &b, &c).buttons & BUTTON_CROUCH, 0);
        press(&mut i, &b, &c, "ControlLeft", true);
        assert_eq!(i.command(Tick(2), &b, &c).buttons & BUTTON_CROUCH, 0);
    }

    #[test]
    fn jump_without_hold_is_sent_once_and_reload_always() {
        let (mut i, b, mut c) = setup();
        c.hold_jump = false;
        press(&mut i, &b, &c, "Space", true);
        press(&mut i, &b, &c, "KeyR", true);
        let first = i.command(Tick(1), &b, &c);
        assert_ne!(first.buttons & BUTTON_JUMP, 0);
        assert_ne!(first.buttons & BUTTON_RELOAD, 0);
        let second = i.command(Tick(2), &b, &c);
        assert_eq!(second.buttons & (BUTTON_JUMP | BUTTON_RELOAD), 0);
        c.hold_jump = true;
        assert_ne!(i.command(Tick(3), &b, &c).buttons & BUTTON_JUMP, 0);
    }

    #[test]
    fn weapon_slots_and_wheel() {
        let (mut i, b, c) = setup();
        assert_eq!(i.command(Tick(1), &b, &c).weapon_slot, 0);
        press(&mut i, &b, &c, "Digit2", true);
        assert_eq!(i.command(Tick(2), &b, &c).weapon_slot, 1);
        i.wheel(1.0);
        assert_eq!(i.slot(), 0);
        press(&mut i, &b, &c, "Digit3", true);
        assert_eq!(i.slot(), 0, "slot 3 does not exist yet");
    }

    #[test]
    fn mouse_turns_right_and_up_like_the_feel_params() {
        let (mut i, b, c) = setup();
        i.mouse_motion(100.0, 0.0, &b, &c);
        let step = degrees_per_count(c.sensitivity).to_radians();
        let expected = (-100.0 * step).rem_euclid(std::f32::consts::TAU);
        assert!((i.look().yaw - expected).abs() < 1e-5);
        i.mouse_motion(0.0, -50.0, &b, &c);
        assert!(i.look().pitch > 0.0, "moving up looks up");
        let mut inverted = c.clone();
        inverted.invert_y = true;
        let up = i.look().pitch;
        i.mouse_motion(0.0, -50.0, &b, &inverted);
        assert!(i.look().pitch < up, "inverted: moving up looks down");
        i.mouse_motion(0.0, -1.0e6, &b, &c);
        assert!(i.look().pitch <= PITCH_LIMIT + 1e-6);
    }

    #[test]
    fn aim_sensitivity_scales_the_turn_while_aiming() {
        let (mut i, b, mut c) = setup();
        c.aim_sensitivity = 0.5;
        i.set_look(0.0, 0.0);
        i.mouse_motion(0.0, 100.0, &b, &c);
        let free = i.look().pitch;
        i.set_look(0.0, 0.0);
        i.press(Input::Mouse(1), true, &b, &c);
        i.mouse_motion(0.0, 100.0, &b, &c);
        assert!((i.look().pitch - free * 0.5).abs() < 1e-5);
    }

    #[test]
    fn the_command_carries_the_quantized_look() {
        let (mut i, b, c) = setup();
        i.set_look(1.0, 0.3);
        let cmd = i.command(Tick(9), &b, &c);
        assert_eq!(cmd.tick, Tick(9));
        assert_eq!(cmd.yaw, quantize_yaw(1.0));
        assert!((dequantize_pitch_command(cmd.pitch) - 0.3).abs() < 1e-3);
    }

    #[test]
    fn releasing_everything_stops_the_player() {
        let (mut i, b, c) = setup();
        press(&mut i, &b, &c, "KeyW", true);
        i.release_all();
        assert_eq!(i.command(Tick(1), &b, &c).move_y, 0);
    }
}
