//! `bot_fill`: bots keep the player count at a target.

/// Number of bots wanted: `max(0, bot_fill - humans)`. A bot yields its slot
/// when a human joins, a bot is added when a human leaves.
#[must_use]
pub fn plan_bot_fill(humans: usize, bot_fill: usize) -> usize {
    bot_fill.saturating_sub(humans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bots_fill_the_missing_slots() {
        assert_eq!(plan_bot_fill(3, 8), 5);
        // A human joining removes one bot.
        assert_eq!(plan_bot_fill(4, 8), 4);
        assert_eq!(plan_bot_fill(0, 8), 8);
        assert_eq!(plan_bot_fill(8, 8), 0);
        assert_eq!(plan_bot_fill(12, 8), 0);
        assert_eq!(plan_bot_fill(5, 0), 0);
    }
}
