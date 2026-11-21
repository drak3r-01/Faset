//! Static mode registry, filled once at startup. No dynamic loading.

use crate::descriptor::ModeDescriptor;
use crate::message::ModeId;
use crate::{GameMode, GameModeClient};

/// A registration that cannot be accepted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// Another mode already uses this descriptor id.
    #[error("mode `{0}` is already registered")]
    Duplicate(&'static str),
    /// More modes than a [`ModeId`] can number.
    #[error("too many modes")]
    Full,
}

/// One registered mode: its descriptor and its constructors.
#[derive(Debug, Clone)]
pub struct ModeEntry {
    /// Numeric id, in registration order.
    pub id: ModeId,
    /// The mode's [`GameMode::descriptor`].
    pub descriptor: ModeDescriptor,
    server: fn() -> Box<dyn GameMode>,
    client: fn() -> Box<dyn GameModeClient>,
}

impl ModeEntry {
    /// A fresh server-side instance.
    #[must_use]
    pub fn create_server(&self) -> Box<dyn GameMode> {
        (self.server)()
    }

    /// A fresh client-side instance.
    #[must_use]
    pub fn create_client(&self) -> Box<dyn GameModeClient> {
        (self.client)()
    }
}

/// Every mode known to this build.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    entries: Vec<ModeEntry>,
}

fn make_server<M: GameMode + Default + 'static>() -> Box<dyn GameMode> {
    Box::new(M::default())
}

fn make_client<M: GameModeClient + Default + 'static>() -> Box<dyn GameModeClient> {
    Box::new(M::default())
}

impl Registry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers mode `M`: `registry.register::<RaceMode>()`.
    ///
    /// # Errors
    /// A duplicate descriptor id, or more than 256 modes.
    pub fn register<M>(&mut self) -> Result<ModeId, RegistryError>
    where
        M: GameMode + GameModeClient + Default + 'static,
    {
        let descriptor = M::descriptor();
        if self.find(descriptor.id).is_some() {
            return Err(RegistryError::Duplicate(descriptor.id));
        }
        let id = ModeId(u8::try_from(self.entries.len()).map_err(|_| RegistryError::Full)?);
        self.entries.push(ModeEntry {
            id,
            descriptor,
            server: make_server::<M>,
            client: make_client::<M>,
        });
        Ok(id)
    }

    /// The mode with numeric id `id`.
    #[must_use]
    pub fn get(&self, id: ModeId) -> Option<&ModeEntry> {
        self.entries.get(usize::from(id.0))
    }

    /// The mode with descriptor id `key`.
    #[must_use]
    pub fn find(&self, key: &str) -> Option<&ModeEntry> {
        self.entries.iter().find(|e| e.descriptor.id == key)
    }

    /// Every mode, in registration order.
    pub fn iter(&self) -> impl Iterator<Item = &ModeEntry> {
        self.entries.iter()
    }
}
