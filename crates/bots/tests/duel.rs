//! Lot 4.10 acceptance: the Expert bot beats the Easy bot, bots move and fight.

mod common;

use common::{fixture, Arena, Fighter};
use faset_bots::Level;

const MATCH_SECONDS: f64 = 90.0;
const SCORE_LIMIT: u32 = 10;
const MATCHES: u64 = 20;

#[derive(Debug, Default, Clone, Copy)]
struct Outcome {
    kills: [u32; 2],
    shots: [u32; 2],
    hits: [u32; 2],
}

/// One 1v1 on the example map between `levels[0]` (first slot) and
/// `levels[1]`, with fixed seeds; the loser of a tie is nobody.
fn play(levels: [Level; 2], seed: u64) -> Outcome {
    play_for(levels, seed, MATCH_SECONDS)
}

fn play_for(levels: [Level; 2], seed: u64, seconds: f64) -> Outcome {
    let f = fixture();
    let spawn = |i: usize| f.spawns[(seed as usize + i * 3) % f.spawns.len()];
    let mut arena = Arena::new(vec![
        Fighter::new(1, levels[0], seed.wrapping_mul(2) + 1, spawn(0)),
        Fighter::new(2, levels[1], seed.wrapping_mul(2) + 2, spawn(1)),
    ]);
    while arena.time() < seconds && arena.fighters.iter().all(|x| x.kills < SCORE_LIMIT) {
        arena.step();
    }
    let k = |i: usize| &arena.fighters[i];
    Outcome {
        kills: [k(0).kills, k(1).kills],
        shots: [k(0).shots, k(1).shots],
        hits: [k(0).hits, k(1).hits],
    }
}

#[test]
fn expert_beats_easy_20_matches() {
    let (mut expert_wins, mut easy_wins, mut draws) = (0, 0, 0);
    let (mut expert_kills, mut easy_kills) = (0, 0);
    let (mut expert_hits, mut expert_shots) = (0, 0);
    let (mut easy_hits, mut easy_shots) = (0, 0);
    for m in 0..MATCHES {
        // The Expert plays slot 0 in even matches and slot 1 in odd ones.
        let expert_slot = (m % 2) as usize;
        let mut levels = [Level::Easy; 2];
        levels[expert_slot] = Level::Expert;
        let o = play(levels, 1000 + m);
        let (e, z) = (o.kills[expert_slot], o.kills[1 - expert_slot]);
        expert_kills += e;
        easy_kills += z;
        expert_hits += o.hits[expert_slot];
        expert_shots += o.shots[expert_slot];
        easy_hits += o.hits[1 - expert_slot];
        easy_shots += o.shots[1 - expert_slot];
        match e.cmp(&z) {
            std::cmp::Ordering::Greater => expert_wins += 1,
            std::cmp::Ordering::Less => easy_wins += 1,
            std::cmp::Ordering::Equal => draws += 1,
        }
        println!("match {m:2}: expert {e:2} - easy {z:2}");
    }
    println!(
        "expert_beats_easy_20_matches: Expert {expert_wins} wins, Easy {easy_wins} wins, \
         {draws} draws; kills {expert_kills} - {easy_kills}; accuracy Expert {expert_hits}/{expert_shots}, \
         Easy {easy_hits}/{easy_shots}"
    );
    assert!(
        expert_wins > easy_wins,
        "Expert {expert_wins} wins vs Easy {easy_wins} ({draws} draws)"
    );
}

#[test]
fn matches_are_reproducible() {
    let a = play_for([Level::Hard, Level::Normal], 77, 25.0);
    let b = play_for([Level::Hard, Level::Normal], 77, 25.0);
    assert_eq!(
        (a.kills, a.shots, a.hits),
        (b.kills, b.shots, b.hits),
        "same seeds, same match"
    );
}

#[test]
fn a_bot_patrols_without_standing_still() {
    let f = fixture();
    let mut arena = Arena::new(vec![Fighter::new(1, Level::Normal, 5, f.spawns[0])]);
    let mut travelled = 0.0;
    let mut last = arena.fighters[0].ps.feet();
    for _ in 0..(60 * 20) {
        arena.step();
        let p = arena.fighters[0].ps.feet();
        travelled += (p - last).length();
        last = p;
    }
    assert!(travelled > 40.0, "only {travelled} m in 20 s");
}

#[test]
fn bots_engage_and_land_shots() {
    let f = fixture();
    let mut arena = Arena::new(vec![
        Fighter::new(1, Level::Expert, 11, f.spawns[0]),
        Fighter::new(2, Level::Normal, 12, f.spawns[3]),
    ]);
    for _ in 0..(60 * 40) {
        arena.step();
    }
    let shots: u32 = arena.fighters.iter().map(|x| x.shots).sum();
    let hits: u32 = arena.fighters.iter().map(|x| x.hits).sum();
    assert!(shots > 10, "bots never shot ({shots})");
    assert!(hits > 0, "bots never hit ({hits} of {shots})");
}

#[test]
fn harder_levels_win_more_often() {
    // Each stronger level beats the weaker one over 4 matches of 45 s (sides swapped).
    for (strong, weak) in [
        (Level::Normal, Level::Easy),
        (Level::Hard, Level::Normal),
        (Level::Expert, Level::Hard),
    ] {
        let (mut s, mut w) = (0, 0);
        for m in 0..4u64 {
            let slot = (m % 2) as usize;
            let mut levels = [weak; 2];
            levels[slot] = strong;
            let o = play_for(levels, 5000 + m, 45.0);
            s += o.kills[slot];
            w += o.kills[1 - slot];
        }
        println!("{strong:?} vs {weak:?}: kills {s} - {w}");
        assert!(s > w, "{strong:?} {s} kills vs {weak:?} {w}");
    }
}

#[test]
fn movement_flags_gate_slides_and_hops() {
    let f = fixture();
    let run = |level| {
        let mut arena = Arena::new(vec![Fighter::new(1, level, 21, f.spawns[0])]);
        for _ in 0..(60 * 60) {
            arena.step();
        }
        (arena.fighters[0].jump_ticks, arena.fighters[0].crouch_ticks)
    };
    let (easy_jump, easy_crouch) = run(Level::Easy);
    let (_, normal_crouch) = run(Level::Normal);
    let (expert_jump, expert_crouch) = run(Level::Expert);
    println!(
        "jump/crouch ticks: easy {easy_jump}/{easy_crouch}, expert {expert_jump}/{expert_crouch}"
    );
    assert_eq!(
        (easy_crouch, normal_crouch),
        (0, 0),
        "no slide without the flag"
    );
    assert!(expert_crouch > 0, "Expert slides on straight lines");
    assert!(expert_jump > easy_jump, "Expert hops");
}
