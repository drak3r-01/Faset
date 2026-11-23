//! Lot 6.3: the server revalidates every loadout message and replaces an
//! invalid one with the default class, telling the sender.

use faset_core::glam::Vec3;
use faset_core::NetId;
use faset_minigame_api::{GameEvent, GameModeClient, Match, MatchConfig, ModeMessage, Registry};
use faset_mode_fps::class::Catalog;
use faset_mode_fps::{register_modes, ClassMsg, FfaMode, Loadout, LoadoutMsg, ServerMsg, FFA_ID};
use faset_sim::{StaticWorld, World};

struct Rig {
    m: Match,
    world: World,
    mode: faset_minigame_api::ModeId,
    catalog: Catalog,
}

impl Rig {
    fn new() -> Self {
        let mut registry = Registry::new();
        register_modes(&mut registry).unwrap();
        let entry = registry.find(FFA_ID).unwrap().clone();
        let config = MatchConfig {
            spawn_points: vec![Vec3::new(0.0, 0.9, 0.0)],
            ..MatchConfig::default()
        };
        let mut m = Match::new(&entry, config).unwrap();
        let mut world = World::with_statics(1, StaticWorld::test_arena());
        world.spawn_player(NetId(1), Vec3::new(0.0, 0.9, 0.0));
        m.player_joined(&world, NetId(1));
        Self {
            m,
            world,
            mode: entry.id,
            catalog: Catalog::builtin(),
        }
    }

    /// Sends `bytes`, ticks, returns the replies about player 1 and the
    /// class index the replicated state shows for it.
    fn send(&mut self, bytes: Vec<u8>) -> (Vec<u8>, Option<u8>) {
        self.m.drain_events();
        let msg = ModeMessage {
            mode: self.mode,
            bytes,
        };
        assert!(self.m.client_message(NetId(1), msg));
        self.world.step(&[]);
        self.m.tick(&self.world);
        let replies = self
            .m
            .drain_events()
            .into_iter()
            .filter_map(|e| match e {
                GameEvent::Mode(b) => match ServerMsg::decode(&b) {
                    Some(ServerMsg::LoadoutReplaced {
                        player: NetId(1),
                        reason,
                    }) => Some(reason),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        let mut client = FfaMode::default();
        client.apply_state(&self.m.state().mode_state);
        (replies, client.view(NetId(1)).and_then(|v| v.class))
    }

    fn class(&self, id: &str) -> u8 {
        u8::try_from(
            self.catalog
                .classes()
                .iter()
                .position(|c| c.id == id)
                .unwrap(),
        )
        .unwrap()
    }
}

fn loadout(class: &str, primary: &str, gadget: Option<&str>) -> Loadout {
    Loadout {
        class: class.into(),
        primary: primary.into(),
        secondary: Some("sidearm".into()),
        melee: Some("baguette".into()),
        gadget: gadget.map(Into::into),
    }
}

#[test]
fn valid_loadout_is_kept_without_a_reply() {
    let mut rig = Rig::new();
    let l = loadout("prankster", "smg", Some("bubble_gun"));
    let msg = l.to_msg(&rig.catalog).unwrap();
    let (replies, class) = rig.send(msg.encode());
    assert!(replies.is_empty());
    assert_eq!(class, Some(rig.class("prankster")));
}

#[test]
fn over_budget_loadout_becomes_trooper_with_a_reply() {
    let mut rig = Rig::new();
    let l = loadout("prankster", "precision_rifle", Some("toaster"));
    let msg = l.to_msg(&rig.catalog).unwrap();
    let (replies, class) = rig.send(msg.encode());
    assert_eq!(replies, [5], "over budget");
    assert_eq!(class, Some(rig.class("trooper")));
}

#[test]
fn crafted_messages_are_replaced_too() {
    let mut rig = Rig::new();
    // Pick prankster first so the replacement is visible.
    let ok = loadout("prankster", "smg", None)
        .to_msg(&rig.catalog)
        .unwrap();
    assert_eq!(rig.send(ok.encode()).1, Some(rig.class("prankster")));
    // Class index out of range.
    let wild = LoadoutMsg { class: 99, ..ok };
    let (replies, class) = rig.send(wild.encode());
    assert_eq!(replies, [6]);
    assert_eq!(class, Some(rig.class("trooper")));
    // Truncated message carrying the loadout tag.
    rig.send(ok.encode());
    let (replies, class) = rig.send(vec![LoadoutMsg::TAG, 1]);
    assert_eq!(replies, [6]);
    assert_eq!(class, Some(rig.class("trooper")));
    // A weapon in the wrong slot (a sidearm as primary).
    let sidearm = u8::try_from(
        rig.catalog
            .weapons()
            .position(|w| w.id == "sidearm")
            .unwrap(),
    )
    .unwrap();
    rig.send(ok.encode());
    let (replies, class) = rig.send(
        LoadoutMsg {
            primary: sidearm,
            ..ok
        }
        .encode(),
    );
    assert_eq!(replies, [3]);
    assert_eq!(class, Some(rig.class("trooper")));
}

#[test]
fn legacy_class_message_still_works() {
    let mut rig = Rig::new();
    let marksman = rig.class("marksman");
    let (replies, class) = rig.send(
        ClassMsg {
            class: marksman,
            secondary: None,
        }
        .encode(),
    );
    assert!(replies.is_empty());
    assert_eq!(class, Some(marksman));
}
