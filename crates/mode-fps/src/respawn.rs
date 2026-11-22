//! Respawn choice: far from the enemies and out of their sight.
//!
//! Candidates visible from any enemy are dropped; the rest are ranked by the
//! distance to the nearest enemy and one of the best three is picked
//! uniformly with the deterministic PCG.
//!
//! Decisions:
//! * A candidate is visible from an enemy when the segment from the enemy's
//!   eye to the candidate's eye or to its centre is not blocked by the map.
//! * If every candidate is visible, the choice falls back to the whole
//!   candidate set (a respawn must always happen).
//! * With no enemy at all, the pick is uniform over every candidate.

use faset_core::glam::Vec3;
use faset_server::Hitbox;
use faset_sim::rng::Pcg32;
use faset_sim::StaticWorld;

/// How many of the best candidates the random pick chooses from.
pub const BEST_COUNT: usize = 3;

/// Segments shorter than this are not blocked by anything, metres.
const EPSILON: f32 = 1e-3;

/// Whether `from` sees `to` through the map.
fn clear(statics: &StaticWorld, from: Vec3, to: Vec3) -> bool {
    let delta = to - from;
    let dist = delta.length();
    if dist < EPSILON {
        return true;
    }
    statics
        .ray_cast(from, delta / dist, dist - EPSILON)
        .is_none()
}

/// Whether an enemy standing at `enemy` (capsule centre) can see a player
/// spawning at `candidate` (capsule centre).
#[must_use]
pub fn visible_from(statics: &StaticWorld, enemy: Vec3, candidate: Vec3) -> bool {
    let from = Hitbox::standing(enemy).eye();
    clear(statics, from, Hitbox::standing(candidate).eye()) || clear(statics, from, candidate)
}

/// Picks the spawn point of a respawning player.
///
/// `candidates` and `enemies` are capsule centres. Returns `None` only with
/// no candidate.
pub fn choose_spawn(
    candidates: &[Vec3],
    enemies: &[Vec3],
    statics: &StaticWorld,
    rng: &mut Pcg32,
) -> Option<Vec3> {
    if candidates.is_empty() {
        return None;
    }
    if enemies.is_empty() {
        return Some(candidates[pick(rng, candidates.len())]);
    }
    let hidden: Vec<usize> = (0..candidates.len())
        .filter(|&i| {
            !enemies
                .iter()
                .any(|&e| visible_from(statics, e, candidates[i]))
        })
        .collect();
    let pool: Vec<usize> = if hidden.is_empty() {
        (0..candidates.len()).collect()
    } else {
        hidden
    };
    let nearest = |i: usize| {
        enemies
            .iter()
            .map(|&e| e.distance(candidates[i]))
            .fold(f32::INFINITY, f32::min)
    };
    let mut ranked = pool;
    // Farthest first, the lowest index on a tie.
    ranked.sort_by(|&a, &b| nearest(b).total_cmp(&nearest(a)).then(a.cmp(&b)));
    ranked.truncate(BEST_COUNT);
    Some(candidates[ranked[pick(rng, ranked.len())]])
}

/// Uniform index below `n`.
fn pick(rng: &mut Pcg32, n: usize) -> usize {
    ((rng.next_f32() * n as f32) as usize).min(n - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use faset_sim::Volume;

    /// An 8 m wall along X at z = 0 (x from -20 to 20).
    fn walled() -> StaticWorld {
        StaticWorld::new(vec![Volume::cuboid(
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::new(20.0, 5.0, 0.25),
        )])
    }

    fn p(x: f32, z: f32) -> Vec3 {
        Vec3::new(x, 0.9, z)
    }

    #[test]
    fn a_point_in_sight_is_never_chosen() {
        let statics = walled();
        let enemy = p(0.0, 10.0);
        let visible = p(0.0, 18.0);
        let hidden = p(0.0, -4.0);
        assert!(visible_from(&statics, enemy, visible));
        assert!(!visible_from(&statics, enemy, hidden));
        for seed in 0..200 {
            let mut rng = Pcg32::new(seed, 1);
            let got = choose_spawn(&[visible, hidden], &[enemy], &statics, &mut rng).unwrap();
            assert_eq!(got, hidden, "seed {seed}");
        }
    }

    #[test]
    fn picks_among_the_best_three_by_distance() {
        let statics = StaticWorld::new(vec![]);
        // All out of sight impossible in the open: use the fallback pool.
        let enemy = p(0.0, 0.0);
        let cands: Vec<Vec3> = [2.0, 4.0, 6.0, 8.0, 10.0]
            .iter()
            .map(|&d| p(d, 0.0))
            .collect();
        let mut seen = [false; 5];
        for seed in 0..300 {
            let mut rng = Pcg32::new(seed, 9);
            let got = choose_spawn(&cands, &[enemy], &statics, &mut rng).unwrap();
            let i = cands.iter().position(|&c| c == got).unwrap();
            seen[i] = true;
        }
        assert_eq!(seen, [false, false, true, true, true]);
    }

    #[test]
    fn distance_uses_the_nearest_enemy_and_is_deterministic() {
        let statics = walled();
        let enemies = [p(-15.0, 5.0), p(15.0, 5.0)];
        // Hidden behind the wall: x = 0 is far from both, x = +-14 is near one.
        let cands = [p(0.0, -8.0), p(14.0, -8.0), p(-14.0, -8.0), p(1.0, -8.0)];
        let run = |seed| {
            let mut rng = Pcg32::new(seed, 3);
            choose_spawn(&cands, &enemies, &statics, &mut rng)
        };
        assert_eq!(run(5), run(5));
        // Nearest-enemy distances: 19.9, 19.1, 13.0, 13.0. The three best
        // are the first, last and (lowest index of the tie) second points.
        let mut seen = [false; 4];
        for seed in 0..200 {
            let got = run(seed).unwrap();
            seen[cands.iter().position(|&c| c == got).unwrap()] = true;
        }
        assert_eq!(seen, [true, true, false, true]);
    }

    #[test]
    fn no_enemy_is_uniform_and_no_candidate_is_none() {
        let statics = StaticWorld::new(vec![]);
        let mut rng = Pcg32::new(1, 1);
        assert_eq!(choose_spawn(&[], &[], &statics, &mut rng), None);
        let cands = [p(0.0, 0.0), p(1.0, 0.0), p(2.0, 0.0), p(3.0, 0.0)];
        let mut seen = [false; 4];
        for seed in 0..100 {
            let mut rng = Pcg32::new(seed, 4);
            let got = choose_spawn(&cands, &[], &statics, &mut rng).unwrap();
            seen[cands.iter().position(|&c| c == got).unwrap()] = true;
        }
        assert_eq!(seen, [true; 4]);
    }

    #[test]
    fn falls_back_when_everything_is_visible() {
        let statics = StaticWorld::new(vec![]);
        let mut rng = Pcg32::new(2, 2);
        let got = choose_spawn(&[p(5.0, 0.0)], &[p(0.0, 0.0)], &statics, &mut rng);
        assert_eq!(got, Some(p(5.0, 0.0)));
    }
}
