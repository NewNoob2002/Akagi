//! 14-tile discard search.
//!
//! Given a 14-tile state, enumerate every possible discard, compute the
//! resulting 13-tile analysis, and return ranked candidate lists.
//! Mirrors `CalculateShantenWithImproves14` in the Go reference.

use super::hand::PlayerInfo34;
use super::improves::analyze_13;
use super::result::{DiscardCandidate, Hand14Result};
use super::shanten::{shanten, shanten_from_counts};
use super::tile::Tile34;

/// Build the candidate lists from a 14-tile state.
pub fn analyze_14(info: &PlayerInfo34) -> Hand14Result {
    let cur_shanten = shanten(info);
    let len_div3 = info.tehai_len_div3();

    let mut maintain: Vec<DiscardCandidate> = Vec::new();
    let mut backwards: Vec<DiscardCandidate> = Vec::new();

    for (d, &count) in info.hand.iter().enumerate() {
        let red = if matches!(d, 4 | 13 | 22) {
            info.aka_hand[d / 9]
        } else {
            0
        };
        for is_red in [false, true] {
            if (is_red && red == 0) || (!is_red && count <= red) {
                continue;
            }
            let mut probe = info.clone();
            probe.hand[d] -= 1;
            probe.own_discards.push(Tile34(d as u8));
            if is_red {
                probe.aka_hand[d / 9] -= 1;
                probe.aka_count -= 1;
            }
            let post_shanten = shanten_from_counts(&probe.hand, len_div3);
            let bucket = if post_shanten == cur_shanten {
                Some(&mut maintain)
            } else if post_shanten == cur_shanten + 1 {
                Some(&mut backwards)
            } else {
                None
            };
            if let Some(b) = bucket {
                let result = analyze_13(&probe);
                let mut discard = Tile34(d as u8).to_mjai().to_string();
                if is_red {
                    discard.push('r');
                }
                b.push(DiscardCandidate { discard, result });
            }
        }
    }

    // Sort: prefer larger mixed_waits_score, then larger waits_total, then
    // larger avg_agari_rate as tiebreakers.
    let cmp = |a: &DiscardCandidate, b: &DiscardCandidate| {
        b.result
            .mixed_waits_score
            .partial_cmp(&a.result.mixed_waits_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.result.waits_total.cmp(&a.result.waits_total))
            .then_with(|| {
                b.result
                    .avg_agari_rate
                    .partial_cmp(&a.result.avg_agari_rate)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    };
    maintain.sort_by(cmp);
    backwards.sort_by(cmp);

    Hand14Result {
        shanten: cur_shanten,
        maintain,
        backwards,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::hand::PlayerInfo34Builder;

    #[test]
    fn discarded_wait_becomes_furiten_and_stays_visible() {
        let info = PlayerInfo34Builder::new()
            .add_many(&[
                "1m", "1m", "2m", "3m", "4m", "2p", "3p", "4p", "6p", "7p", "2s", "3s", "4s", "5p",
            ])
            .build();
        let result = analyze_14(&info);
        let candidate = result
            .maintain
            .iter()
            .chain(&result.backwards)
            .find(|c| c.discard == "5p")
            .unwrap();
        assert!(candidate.result.is_furiten);
        assert_eq!(candidate.result.furiten_rate, 1.0);
        assert_eq!(
            candidate
                .result
                .waits
                .iter()
                .find(|w| w.tile == "5p")
                .unwrap()
                .left,
            3
        );
        assert!(info.own_discards.is_empty());
    }

    #[test]
    fn red_and_ordinary_discards_have_distinct_bonuses() {
        let info = PlayerInfo34Builder::new()
            .add_many(&[
                "2m", "3m", "4m", "2p", "3p", "4p", "2s", "3s", "4s", "5p", "5pr", "E", "E", "E",
            ])
            .build();
        let result = analyze_14(&info);
        let candidates: Vec<_> = result.maintain.iter().chain(&result.backwards).collect();
        let ordinary = candidates.iter().find(|c| c.discard == "5p").unwrap();
        let red = candidates.iter().find(|c| c.discard == "5pr").unwrap();
        assert_eq!(ordinary.result.waits_total, red.result.waits_total);
        assert!(ordinary.result.riichi_point > red.result.riichi_point);
        let only_red = PlayerInfo34Builder::new()
            .add_many(&[
                "1m", "1m", "2m", "3m", "4m", "2p", "3p", "4p", "6p", "7p", "2s", "3s", "4s", "5pr",
            ])
            .build();
        let result = analyze_14(&only_red);
        assert!(!result
            .maintain
            .iter()
            .chain(&result.backwards)
            .any(|c| c.discard == "5p"));
    }

    #[test]
    fn fourteen_tiles_produces_candidates() {
        // 234m 234p 234s 678p + EE pair + extra E (14 tiles)
        let info = PlayerInfo34Builder::new()
            .add_many(&[
                "2m", "3m", "4m", "2p", "3p", "4p", "6p", "7p", "8p", "2s", "3s", "4s", "E", "E",
            ])
            .build();
        assert_eq!(info.hand_size(), 14);
        let r = analyze_14(&info);
        assert_eq!(r.shanten, -1, "winning shape should be agari");
    }

    #[test]
    fn discard_candidate_for_one_shanten_14() {
        // 1-shanten 14: 1-9m + 1-2p + N + P + extra (we need 14 tiles)
        // Take the 1-shanten 13 from earlier and add an extra tile to make 14.
        let info = PlayerInfo34Builder::new()
            .add_many(&[
                "1m", "2m", "3m", "4m", "5m", "6m", "7m", "8m", "9m", "1p", "2p", "N", "P", "P",
            ])
            .build();
        assert_eq!(info.hand_size(), 14);
        let r = analyze_14(&info);
        // current shanten = 1 (since the previous 13-state was 1-shanten, adding
        // a 2nd P actually gives a pair → tenpai).
        // Discarding N and keeping the new pair should be the top-rated maintain.
        assert!(r.shanten <= 1);
        assert!(!r.maintain.is_empty());
        // Top candidate should have the highest mixed_waits_score.
        let top = &r.maintain[0];
        for cand in r.maintain.iter().skip(1) {
            assert!(top.result.mixed_waits_score >= cand.result.mixed_waits_score - 1e-9);
        }
    }
}
