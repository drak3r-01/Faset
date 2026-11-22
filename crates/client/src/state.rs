//! The application state machine: a stack of [`AppState`]s.
//!
//! The bottom of the stack is the screen the player returns to
//! (`MainMenu`, or `InGame` while a match runs); states pushed above it
//! (settings, pause menu, loadout) never destroy what lies below, so popping
//! them gives back the same match. The match itself is not owned here: it
//! lives in the application's [`Session`](crate::Session), which only
//! [`StateStack::replace`] and [`StateStack::reset`] (leaving the match) may
//! make go away.

pub use faset_ui::AppState;
use faset_ui::UiCommand;

/// A stack operation that is not allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StateError {
    /// The bottom state cannot be popped.
    #[error("the bottom state cannot be popped")]
    PopBottom,
    /// `Boot` only exists at the very start.
    #[error("`Boot` cannot be pushed")]
    PushBoot,
    /// The pause menu belongs on top of a running match.
    #[error("the pause menu can only be pushed on `InGame`")]
    PauseOutsideGame,
    /// A match is entered by replacing `Loading`, never pushed.
    #[error("`InGame` cannot be pushed")]
    PushInGame,
}

/// What happened to the stack, drained by the application to start and stop
/// per-state work (load a map, open a connection, release a session).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// A state was pushed on top of another.
    Pushed(AppState),
    /// The top state was popped; the state below is revealed.
    Popped(AppState),
    /// The top state was replaced.
    Replaced {
        /// State that left.
        from: AppState,
        /// State that entered.
        to: AppState,
    },
    /// The whole stack was reset to one state.
    Reset(AppState),
}

/// A stack of application states with a log of transitions.
#[derive(Debug, Clone)]
pub struct StateStack {
    stack: Vec<AppState>,
    events: Vec<Transition>,
}

impl Default for StateStack {
    /// A stack holding `Boot`.
    fn default() -> Self {
        Self {
            stack: vec![AppState::Boot],
            events: Vec::new(),
        }
    }
}

impl StateStack {
    /// A stack holding `bottom`.
    #[must_use]
    pub fn new(bottom: AppState) -> Self {
        Self {
            stack: vec![bottom],
            events: Vec::new(),
        }
    }

    /// The active state.
    #[must_use]
    pub fn top(&self) -> AppState {
        *self.stack.last().expect("the stack is never empty")
    }

    /// The states, bottom first.
    #[must_use]
    pub fn states(&self) -> &[AppState] {
        &self.stack
    }

    /// Number of states.
    #[must_use]
    pub fn len(&self) -> usize {
        self.stack.len()
    }

    /// Always false: the stack keeps at least one state.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Whether `state` is anywhere in the stack.
    #[must_use]
    pub fn contains(&self, state: AppState) -> bool {
        self.stack.contains(&state)
    }

    /// Pushes `state` over the current one.
    ///
    /// # Errors
    /// `Boot`, `InGame`, or `Pause` anywhere but on `InGame`.
    pub fn push(&mut self, state: AppState) -> Result<(), StateError> {
        match state {
            AppState::Boot => return Err(StateError::PushBoot),
            AppState::InGame => return Err(StateError::PushInGame),
            AppState::Pause if self.top() != AppState::InGame => {
                return Err(StateError::PauseOutsideGame)
            }
            _ => {}
        }
        self.stack.push(state);
        self.events.push(Transition::Pushed(state));
        Ok(())
    }

    /// Pops the top state and returns it.
    ///
    /// # Errors
    /// [`StateError::PopBottom`] when only one state is left.
    pub fn pop(&mut self) -> Result<AppState, StateError> {
        if self.stack.len() == 1 {
            return Err(StateError::PopBottom);
        }
        let state = self.stack.pop().expect("length checked");
        self.events.push(Transition::Popped(state));
        Ok(state)
    }

    /// Replaces the top state with `state` and returns the old one.
    ///
    /// # Errors
    /// `Boot` is never a target; `Pause` needs `InGame` below it.
    pub fn replace(&mut self, state: AppState) -> Result<AppState, StateError> {
        if state == AppState::Boot {
            return Err(StateError::PushBoot);
        }
        if state == AppState::Pause && self.stack.len() < 2 {
            return Err(StateError::PauseOutsideGame);
        }
        let top = self.stack.last_mut().expect("the stack is never empty");
        let from = std::mem::replace(top, state);
        self.events.push(Transition::Replaced { from, to: state });
        Ok(from)
    }

    /// Drops every state and leaves only `state` (leaving a match).
    pub fn reset(&mut self, state: AppState) {
        self.stack.clear();
        self.stack.push(state);
        self.events.push(Transition::Reset(state));
    }

    /// Applies the stack part of a screen command. Returns the command back
    /// when it is not a stack operation (`Quit`, `StartMatch`, `Connect`,
    /// `LeaveMatch`), for the application to handle.
    ///
    /// # Errors
    /// The stack operation was refused.
    pub fn apply(&mut self, command: UiCommand) -> Result<Option<UiCommand>, StateError> {
        match command {
            UiCommand::Push(s) => self.push(s).map(|()| None),
            UiCommand::Pop => self.pop().map(|_| None),
            UiCommand::Replace(s) => self.replace(s).map(|_| None),
            other => Ok(Some(other)),
        }
    }

    /// Takes the transitions since the last call, oldest first.
    pub fn drain_events(&mut self) -> Vec<Transition> {
        std::mem::take(&mut self.events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules() {
        let mut s = StateStack::new(AppState::MainMenu);
        assert_eq!(s.pop(), Err(StateError::PopBottom));
        assert_eq!(s.push(AppState::Boot), Err(StateError::PushBoot));
        assert_eq!(s.push(AppState::InGame), Err(StateError::PushInGame));
        assert_eq!(s.push(AppState::Pause), Err(StateError::PauseOutsideGame));
        assert_eq!(s.len(), 1);
        assert!(s.drain_events().is_empty());
    }

    #[test]
    fn events_are_recorded_in_order() {
        let mut s = StateStack::new(AppState::MainMenu);
        s.push(AppState::Settings).unwrap();
        s.replace(AppState::Loadout).unwrap();
        s.pop().unwrap();
        s.reset(AppState::MainMenu);
        assert_eq!(
            s.drain_events(),
            [
                Transition::Pushed(AppState::Settings),
                Transition::Replaced {
                    from: AppState::Settings,
                    to: AppState::Loadout
                },
                Transition::Popped(AppState::Loadout),
                Transition::Reset(AppState::MainMenu),
            ]
        );
        assert!(s.drain_events().is_empty());
    }
}
