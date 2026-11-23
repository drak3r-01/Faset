//! Key bindings: every game action maps to one input (a keyboard key or a
//! mouse button), with conflict detection.
//!
//! Inputs are named with the physical key names of the windowing layer
//! (`KeyW`, `Space`, `ShiftLeft`, `Digit1`), so the file stays readable and
//! this crate needs no windowing dependency. Mouse buttons are `Mouse0`
//! (left), `Mouse1` (right), `Mouse2` (middle) and so on; gamepad buttons are
//! `Pad:South`, `Pad:RightTrigger`, ...

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A bindable game action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    /// Move forward.
    Forward,
    /// Move backward.
    Backward,
    /// Strafe left.
    Left,
    /// Strafe right.
    Right,
    /// Jump.
    Jump,
    /// Crouch.
    Crouch,
    /// Sprint.
    Sprint,
    /// Fire the weapon.
    Fire,
    /// Aim down the sights.
    Aim,
    /// Reload.
    Reload,
    /// Use or pick up.
    Interact,
    /// Next weapon.
    NextWeapon,
    /// Previous weapon.
    PreviousWeapon,
    /// Weapon slot 1.
    Slot1,
    /// Weapon slot 2.
    Slot2,
    /// Weapon slot 3.
    Slot3,
    /// Show the scoreboard.
    Scoreboard,
    /// Open the chat.
    Chat,
}

impl Action {
    /// Every action, in menu order.
    pub const ALL: [Self; 18] = [
        Self::Forward,
        Self::Backward,
        Self::Left,
        Self::Right,
        Self::Jump,
        Self::Crouch,
        Self::Sprint,
        Self::Fire,
        Self::Aim,
        Self::Reload,
        Self::Interact,
        Self::NextWeapon,
        Self::PreviousWeapon,
        Self::Slot1,
        Self::Slot2,
        Self::Slot3,
        Self::Scoreboard,
        Self::Chat,
    ];

    /// Stable name, the key of the TOML table.
    pub fn name(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Backward => "backward",
            Self::Left => "left",
            Self::Right => "right",
            Self::Jump => "jump",
            Self::Crouch => "crouch",
            Self::Sprint => "sprint",
            Self::Fire => "fire",
            Self::Aim => "aim",
            Self::Reload => "reload",
            Self::Interact => "interact",
            Self::NextWeapon => "next_weapon",
            Self::PreviousWeapon => "previous_weapon",
            Self::Slot1 => "slot_1",
            Self::Slot2 => "slot_2",
            Self::Slot3 => "slot_3",
            Self::Scoreboard => "scoreboard",
            Self::Chat => "chat",
        }
    }

    /// The action called `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.name() == name)
    }

    /// Translation key of the action's label.
    pub fn label_key(self) -> &'static str {
        match self {
            Self::Forward => "action.forward",
            Self::Backward => "action.backward",
            Self::Left => "action.left",
            Self::Right => "action.right",
            Self::Jump => "action.jump",
            Self::Crouch => "action.crouch",
            Self::Sprint => "action.sprint",
            Self::Fire => "action.fire",
            Self::Aim => "action.aim",
            Self::Reload => "action.reload",
            Self::Interact => "action.interact",
            Self::NextWeapon => "action.next_weapon",
            Self::PreviousWeapon => "action.previous_weapon",
            Self::Slot1 => "action.slot_1",
            Self::Slot2 => "action.slot_2",
            Self::Slot3 => "action.slot_3",
            Self::Scoreboard => "action.scoreboard",
            Self::Chat => "action.chat",
        }
    }

    /// The input this action is bound to by default.
    pub fn default_input(self) -> Input {
        match self {
            Self::Forward => Input::key("KeyW"),
            Self::Backward => Input::key("KeyS"),
            Self::Left => Input::key("KeyA"),
            Self::Right => Input::key("KeyD"),
            Self::Jump => Input::key("Space"),
            Self::Crouch => Input::key("ControlLeft"),
            Self::Sprint => Input::key("ShiftLeft"),
            Self::Fire => Input::Mouse(0),
            Self::Aim => Input::Mouse(1),
            Self::Reload => Input::key("KeyR"),
            Self::Interact => Input::key("KeyE"),
            Self::NextWeapon => Input::key("KeyF"),
            Self::PreviousWeapon => Input::key("KeyQ"),
            Self::Slot1 => Input::key("Digit1"),
            Self::Slot2 => Input::key("Digit2"),
            Self::Slot3 => Input::key("Digit3"),
            Self::Scoreboard => Input::key("Tab"),
            Self::Chat => Input::key("KeyT"),
        }
    }
}

/// Name of the key that cannot be bound: it always opens the pause menu.
pub const RESERVED_KEY: &str = "Escape";

/// A physical input: a keyboard key, a mouse button or a gamepad button.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Input {
    /// A keyboard key, by physical key name.
    Key(String),
    /// A mouse button (0 left, 1 right, 2 middle, then the side buttons).
    Mouse(u8),
    /// A gamepad button, by name.
    Pad(String),
}

impl Input {
    /// A keyboard key.
    pub fn key(name: &str) -> Self {
        Self::Key(name.to_owned())
    }

    /// True for the reserved key.
    pub fn is_reserved(&self) -> bool {
        matches!(self, Self::Key(k) if k == RESERVED_KEY)
    }

    /// Short text for menus (`KeyW` shows as `W`, `Mouse0` as `Mouse 1`).
    pub fn label(&self) -> String {
        match self {
            Self::Key(k) => k
                .strip_prefix("Key")
                .or_else(|| k.strip_prefix("Digit"))
                .unwrap_or(k)
                .to_owned(),
            Self::Mouse(n) => format!("Mouse {}", u32::from(*n) + 1),
            Self::Pad(p) => format!("Pad {p}"),
        }
    }
}

impl fmt::Display for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Key(k) => f.write_str(k),
            Self::Mouse(n) => write!(f, "Mouse{n}"),
            Self::Pad(p) => write!(f, "Pad:{p}"),
        }
    }
}

/// An input name that could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid input name `{0}`")]
pub struct ParseInputError(String);

impl FromStr for Input {
    type Err = ParseInputError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || ParseInputError(s.to_owned());
        if let Some(pad) = s.strip_prefix("Pad:") {
            return if pad.is_empty() {
                Err(bad())
            } else {
                Ok(Self::Pad(pad.to_owned()))
            };
        }
        if let Some(n) = s.strip_prefix("Mouse") {
            return n.parse().map(Self::Mouse).map_err(|_| bad());
        }
        if s.is_empty() || !s.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(bad());
        }
        Ok(Self::Key(s.to_owned()))
    }
}

/// Result of [`Bindings::assign`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assign {
    /// The action now uses the input.
    Bound,
    /// Another action already uses the input; nothing changed. The caller
    /// asks the player to swap or cancels.
    Conflict(Action),
    /// The input cannot be bound (the pause key).
    Reserved,
}

/// Action to input table. Each action has at most one input and each input
/// at most one action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "BTreeMap<String, String>", into = "BTreeMap<String, String>")]
pub struct Bindings {
    map: BTreeMap<Action, Input>,
}

impl Default for Bindings {
    fn default() -> Self {
        Self {
            map: Action::ALL
                .into_iter()
                .map(|a| (a, a.default_input()))
                .collect(),
        }
    }
}

impl From<BTreeMap<String, String>> for Bindings {
    /// Reads a hand-edited table: unknown actions, unreadable names, the
    /// reserved key and repeated inputs are dropped; actions the file does
    /// not mention keep their default input when it is still free.
    fn from(table: BTreeMap<String, String>) -> Self {
        let mut map = BTreeMap::new();
        for action in Action::ALL {
            if let Some(input) = table
                .get(action.name())
                .and_then(|s| s.parse::<Input>().ok())
                .filter(|i| !i.is_reserved() && !map.values().any(|v| v == i))
            {
                map.insert(action, input);
            }
        }
        for action in Action::ALL {
            let default = action.default_input();
            if !table.contains_key(action.name()) && !map.values().any(|v| *v == default) {
                map.insert(action, default);
            }
        }
        Self { map }
    }
}

impl From<Bindings> for BTreeMap<String, String> {
    fn from(b: Bindings) -> Self {
        b.map
            .into_iter()
            .map(|(a, i)| (a.name().to_owned(), i.to_string()))
            .collect()
    }
}

impl Bindings {
    /// The input of `action`, if bound.
    pub fn get(&self, action: Action) -> Option<&Input> {
        self.map.get(&action)
    }

    /// The action that uses `input`.
    pub fn action_of(&self, input: &Input) -> Option<Action> {
        self.map.iter().find(|(_, i)| *i == input).map(|(a, _)| *a)
    }

    /// The other action that already uses `input`, if binding it to
    /// `action` would conflict.
    pub fn conflict(&self, action: Action, input: &Input) -> Option<Action> {
        self.action_of(input).filter(|other| *other != action)
    }

    /// Binds `input` to `action` unless another action uses it or it is
    /// reserved; a conflict changes nothing.
    pub fn assign(&mut self, action: Action, input: Input) -> Assign {
        if input.is_reserved() {
            return Assign::Reserved;
        }
        if let Some(other) = self.conflict(action, &input) {
            return Assign::Conflict(other);
        }
        self.map.insert(action, input);
        Assign::Bound
    }

    /// Resolves a conflict by swapping: `action` takes `input` and `other`
    /// takes the input `action` had (or becomes unbound if it had none).
    pub fn swap(&mut self, action: Action, input: Input, other: Action) {
        let previous = self.map.remove(&action);
        self.map.remove(&other);
        self.map.insert(action, input);
        if let Some(previous) = previous {
            self.map.insert(other, previous);
        }
    }

    /// Leaves `action` without an input.
    pub fn unbind(&mut self, action: Action) {
        self.map.remove(&action);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_no_conflict_and_no_reserved_key() {
        let b = Bindings::default();
        for a in Action::ALL {
            let input = b.get(a).unwrap_or_else(|| panic!("{a:?} unbound"));
            assert!(!input.is_reserved());
            assert_eq!(b.action_of(input), Some(a));
        }
    }

    #[test]
    fn rebinding_a_bound_key_reports_the_conflict() {
        let mut b = Bindings::default();
        let w = Input::key("KeyW");
        assert_eq!(
            b.assign(Action::Jump, w.clone()),
            Assign::Conflict(Action::Forward)
        );
        // Nothing changed.
        assert_eq!(b.get(Action::Jump), Some(&Input::key("Space")));
        assert_eq!(b.get(Action::Forward), Some(&w));
        // Rebinding to its own key is not a conflict.
        assert_eq!(b.assign(Action::Forward, w), Assign::Bound);
    }

    #[test]
    fn free_key_binds_and_reserved_key_is_refused() {
        let mut b = Bindings::default();
        assert_eq!(b.assign(Action::Jump, Input::key("KeyZ")), Assign::Bound);
        assert_eq!(b.get(Action::Jump), Some(&Input::key("KeyZ")));
        assert_eq!(
            b.assign(Action::Jump, Input::key(RESERVED_KEY)),
            Assign::Reserved
        );
    }

    #[test]
    fn swap_exchanges_the_two_inputs() {
        let mut b = Bindings::default();
        b.swap(Action::Jump, Input::key("KeyW"), Action::Forward);
        assert_eq!(b.get(Action::Jump), Some(&Input::key("KeyW")));
        assert_eq!(b.get(Action::Forward), Some(&Input::key("Space")));
    }

    #[test]
    fn input_names_round_trip() {
        for s in ["KeyW", "Space", "Mouse0", "Mouse4", "Pad:South", "Digit1"] {
            assert_eq!(s.parse::<Input>().unwrap().to_string(), s);
        }
        assert!("".parse::<Input>().is_err());
        assert!("MouseX".parse::<Input>().is_err());
        assert!("Pad:".parse::<Input>().is_err());
        assert_eq!(Input::key("KeyW").label(), "W");
        assert_eq!(Input::Mouse(0).label(), "Mouse 1");
    }

    #[test]
    fn toml_round_trip_and_hand_edits() {
        #[derive(Serialize, Deserialize)]
        struct File {
            bindings: Bindings,
        }
        let mut b = Bindings::default();
        b.swap(Action::Jump, Input::key("KeyW"), Action::Forward);
        let text = toml::to_string(&File {
            bindings: b.clone(),
        })
        .unwrap();
        let back: File = toml::from_str(&text).unwrap();
        assert_eq!(back.bindings, b);

        // Repeated input, reserved key, unknown action: dropped; the missing
        // actions keep their default when free.
        let edited: File = toml::from_str(
            "[bindings]\njump = \"KeyW\"\nforward = \"KeyW\"\nfire = \"Escape\"\nnope = \"KeyZ\"\n",
        )
        .unwrap();
        let e = edited.bindings;
        assert_eq!(e.get(Action::Forward), Some(&Input::key("KeyW")));
        assert_eq!(e.get(Action::Jump), None);
        assert_eq!(e.get(Action::Fire), None);
        assert_eq!(e.get(Action::Reload), Some(&Input::key("KeyR")));
    }
}
