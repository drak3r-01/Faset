//! The join screen (lot 6.2): the LAN list, a connect-by-address field,
//! favourites and history.
//!
//! [`ServerListModel`] is the view model of the LAN list. The application
//! feeds it the servers it saw and how long ago each one last answered; the
//! model advances on a clock it is given (so expiry is testable without
//! waiting) and drops an entry [`ENTRY_TTL`] after its last reply.

use std::time::Duration;

use crate::screens::{back, panel};
use crate::{text, MenuModel, ServerRow, Theme, UiCommand};

/// An entry expires this long after the server's last reply.
pub const ENTRY_TTL: Duration = Duration::from_secs(6);

/// A server of the LAN list.
#[derive(Debug, Clone, PartialEq)]
pub struct BrowserEntry {
    /// What the join screen shows.
    pub row: ServerRow,
    /// Model time of the server's latest reply.
    last_seen: Duration,
}

/// The LAN list with its expiry.
#[derive(Debug, Clone, Default)]
pub struct ServerListModel {
    now: Duration,
    entries: Vec<BrowserEntry>,
}

impl ServerListModel {
    /// Advances the clock by `dt` and drops the entries whose last reply is
    /// [`ENTRY_TTL`] old.
    pub fn advance(&mut self, dt: Duration) {
        self.now += dt;
        let now = self.now;
        self.entries
            .retain(|e| now.saturating_sub(e.last_seen) < ENTRY_TTL);
    }

    /// Records that the server at `row.address` answered `age` ago. Keeps
    /// the first-seen order; an entry already listed is updated.
    pub fn observe(&mut self, row: ServerRow, age: Duration) {
        let last_seen = self.now.saturating_sub(age);
        match self
            .entries
            .iter_mut()
            .find(|e| e.row.address == row.address)
        {
            Some(e) => {
                e.row = row;
                e.last_seen = e.last_seen.max(last_seen);
            }
            None => self.entries.push(BrowserEntry { row, last_seen }),
        }
        let now = self.now;
        self.entries
            .retain(|e| now.saturating_sub(e.last_seen) < ENTRY_TTL);
    }

    /// The listed servers.
    #[must_use]
    pub fn entries(&self) -> &[BrowserEntry] {
        &self.entries
    }

    /// The rows the join screen draws.
    #[must_use]
    pub fn rows(&self) -> Vec<ServerRow> {
        self.entries.iter().map(|e| e.row.clone()).collect()
    }
}

fn favourite_button(
    ui: &mut egui::Ui,
    model: &MenuModel,
    address: &str,
    cmds: &mut Vec<UiCommand>,
) {
    let key = if model.favourites.iter().any(|a| a == address) {
        "join.unfavourite"
    } else {
        "join.favourite"
    };
    if ui.small_button(text(key)).clicked() {
        cmds.push(UiCommand::ToggleFavourite(address.to_owned()));
    }
}

/// Join screen: LAN servers, connect by address, favourites and history.
pub fn server_browser(
    ctx: &egui::Context,
    theme: &Theme,
    model: &mut MenuModel,
    cmds: &mut Vec<UiCommand>,
) {
    panel(ctx, theme, text("join.title"), |ui| {
        ui.label(text("join.lan"));
        if model.lan_servers.is_empty() {
            ui.colored_label(theme.text_muted, text("join.none"));
        }
        for row in &model.lan_servers {
            ui.horizontal(|ui| {
                let label = format!(
                    "{}  {}  {}/{}",
                    row.name, row.map, row.players.0, row.players.1
                );
                if ui.button(label).clicked() {
                    cmds.push(UiCommand::Connect(row.address.clone()));
                }
                favourite_button(ui, model, &row.address, cmds);
            });
        }
        ui.add_space(8.0);
        ui.label(text("join.address"));
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut model.address);
            let address = model.address.trim().to_owned();
            if ui.button(text("join.connect")).clicked() && !address.is_empty() {
                cmds.push(UiCommand::Connect(address.clone()));
            }
            if !address.is_empty() {
                favourite_button(ui, model, &address, cmds);
            }
        });
        for (key, list) in [
            ("join.favourites", &model.favourites),
            ("join.history", &model.history),
        ] {
            ui.add_space(8.0);
            ui.label(text(key));
            if list.is_empty() {
                ui.colored_label(theme.text_muted, text("join.none"));
            }
            for address in list {
                ui.horizontal(|ui| {
                    if ui.button(address).clicked() {
                        cmds.push(UiCommand::Connect(address.clone()));
                    }
                    favourite_button(ui, model, address, cmds);
                });
            }
        }
        back(ui, cmds);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(address: &str) -> ServerRow {
        ServerRow {
            name: "game".into(),
            address: address.into(),
            players: (1, 16),
            map: "example".into(),
        }
    }

    #[test]
    fn an_entry_disappears_six_seconds_after_its_last_reply() {
        let mut list = ServerListModel::default();
        list.observe(row("10.0.0.2:7777"), Duration::ZERO);
        list.advance(Duration::from_millis(5900));
        assert_eq!(list.entries().len(), 1);
        list.advance(Duration::from_millis(100));
        assert!(list.entries().is_empty());
    }

    #[test]
    fn a_new_reply_keeps_the_entry_and_restarts_its_timer() {
        let mut list = ServerListModel::default();
        list.observe(row("a:1"), Duration::ZERO);
        list.advance(Duration::from_secs(4));
        list.observe(row("a:1"), Duration::ZERO);
        list.observe(row("b:1"), Duration::from_secs(5));
        list.advance(Duration::from_secs(4));
        assert_eq!(
            list.rows().iter().map(|r| &*r.address).collect::<Vec<_>>(),
            ["a:1"]
        );
    }

    #[test]
    fn an_entry_older_than_the_limit_is_not_listed() {
        let mut list = ServerListModel::default();
        list.advance(Duration::from_secs(10));
        list.observe(row("a:1"), Duration::from_secs(7));
        assert!(list.entries().is_empty());
    }
}
