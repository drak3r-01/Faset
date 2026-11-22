//! Lot 4.10 acceptance: 16 bots in under 1 ms per tick.

mod common;

use common::{fixture, Arena, Fighter};

#[test]
fn sixteen_bots_cost_under_a_millisecond_per_tick() {
    let f = fixture();
    let fighters = (0..16u16)
        .map(|i| {
            let level = faset_bots::Level::ALL[usize::from(i) % 4];
            Fighter::new(
                i + 1,
                level,
                100 + u64::from(i),
                f.spawns[usize::from(i) % f.spawns.len()],
            )
        })
        .collect();
    let mut arena = Arena::new(fighters);
    // Warm up: first plans, spread over the staggered phases.
    for _ in 0..120 {
        arena.step();
    }
    arena.time_bots = true;
    let ticks = 600;
    for _ in 0..ticks {
        arena.step();
    }
    let calls = arena.bot_calls.max(1);
    let per_bot = arena.bot_time.as_secs_f64() / calls as f64;
    let alive_avg = calls as f64 / f64::from(ticks);
    let per_tick_16_ms = per_bot * 16.0 * 1e3;
    println!(
        "budget: {} bots alive on average, {:.1} us per bot, {:.3} ms per tick for 16 bots \
         (debug assertions: {})",
        alive_avg,
        per_bot * 1e6,
        per_tick_16_ms,
        cfg!(debug_assertions)
    );
    // The 1 ms budget is for optimised builds; an unoptimised test build is
    // only checked against a loose bound.
    let limit = if cfg!(debug_assertions) { 40.0 } else { 1.0 };
    assert!(
        per_tick_16_ms < limit,
        "16 bots cost {per_tick_16_ms:.3} ms per tick (limit {limit} ms)"
    );
}
