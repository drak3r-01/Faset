//! What an application needs from the client side of an FPS mode (lot 6.6):
//! the HUD, the replicated players and the pickups, behind one object-safe
//! trait, so the client does not care which mode it mirrors.

use faset_core::glam::Vec3;
use faset_core::NetId;
use faset_minigame_api::GameModeClient;

use crate::class::ClassDef;
use crate::hud::{HudInput, HudModel};
use crate::mode::FpsMode;
use crate::pickup::PickupKind;
use crate::play::{FfaMode, TeamMode};
use crate::weapon::WeaponDef;

/// A player as the replicated state shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientPlayer {
    pub alive: bool,
    pub health: u16,
    pub armour: u16,
    /// Team index, in team modes.
    pub team: Option<u8>,
    /// Index of the class in the catalog.
    pub class: Option<u8>,
    /// Index of the weapon in hand in [`FpsClient::weapons`].
    pub weapon: Option<usize>,
    /// Rounds in the magazine of the weapon in hand.
    pub ammo: u16,
    /// Rounds outside the magazine.
    pub reserve: u16,
}

/// A pickup of the map as the replicated state shows it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClientPickup {
    pub kind: PickupKind,
    pub position: Vec3,
    pub available: bool,
}

/// The client side of an FPS mode.
pub trait FpsClient {
    /// Sets what the host knows for the HUD (see [`HudInput`]).
    fn set_hud_input(&mut self, input: HudInput);
    /// Ages the HUD's feed, markers and damage numbers.
    fn advance_hud(&mut self, dt: f32);
    /// The HUD data of this frame.
    fn hud_model(&self) -> HudModel;
    /// Every player of the replicated state.
    fn players(&self) -> Vec<(NetId, ClientPlayer)>;
    /// The pickups of the map.
    fn pickups(&self) -> Vec<ClientPickup>;
    /// The weapons of the match, in wire order.
    fn weapons(&self) -> &[WeaponDef];
    /// The classes of the match, in wire order.
    fn classes(&self) -> &[ClassDef];
}

/// The FPS client behind `mode`, if it is one of this crate's modes.
pub fn fps_client(mode: &mut dyn GameModeClient) -> Option<&mut dyn FpsClient> {
    let any = mode.as_any_mut()?;
    if any.is::<FfaMode>() {
        return any
            .downcast_mut::<FfaMode>()
            .map(|m| m as &mut dyn FpsClient);
    }
    if any.is::<TeamMode>() {
        return any
            .downcast_mut::<TeamMode>()
            .map(|m| m as &mut dyn FpsClient);
    }
    any.downcast_mut::<FpsMode>()
        .map(|m| m as &mut dyn FpsClient)
}
