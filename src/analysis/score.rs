//! Score / point expectation.
//!
//! Wraps `riichienv-core::HandEvaluator::calc` to compute ron-point estimates
//! for a tenpai hand. We probe each wait tile under two conditions:
//!   - dama (no riichi),
//!   - riichi (closed hands only).
//!
//! Ura-dora is ignored (no information at decision time). Yakuhai-only
//! correctness depends on round/seat winds being passed through.

use riichienv_core::hand_evaluator::HandEvaluator;
use riichienv_core::types::{Conditions, Meld as RiMeld, MeldType as RiMeldType, Wind};

use super::hand::{Meld34, Meld34Kind, PlayerInfo34};
use super::tile::{Tile34, HONOR_BASE};
use super::waits::Waits;

/// Per-wait point breakdown returned by [`expectation`].
#[derive(Debug, Clone, Default)]
pub struct ScoreEstimate {
    /// Average dama (no riichi) ron point, weighted by remaining-tile count.
    pub dama_point: f64,
    /// Average riichi ron point. Zero if the hand is open.
    pub riichi_point: f64,
    /// Yaku ids encountered across all waits (union, deduplicated).
    pub yaku_ids: Vec<u32>,
    /// Whether at least one wait produces a valid (≥1 han) winning hand.
    pub has_yaku: bool,
}

fn wind_from_tile(tile: Tile34) -> Wind {
    let off = tile.idx().saturating_sub(HONOR_BASE);
    Wind::from(off)
}

fn meld_to_riichienv(m: &Meld34) -> RiMeld {
    let mt = match m.kind {
        Meld34Kind::Chi => RiMeldType::Chi,
        Meld34Kind::Pon => RiMeldType::Pon,
        Meld34Kind::Daiminkan => RiMeldType::Daiminkan,
        Meld34Kind::Ankan => RiMeldType::Ankan,
        Meld34Kind::Kakan => RiMeldType::Kakan,
    };
    let mut tiles_136: Vec<u8> = Vec::new();
    match m.kind {
        Meld34Kind::Chi => {
            for (i, t) in m.tiles.iter().enumerate() {
                tiles_136.push(t.idx() * 4 + (i as u8));
            }
        }
        Meld34Kind::Pon => {
            if let Some(t) = m.tiles.first() {
                for k in 0..3 {
                    tiles_136.push(t.idx() * 4 + k);
                }
            }
        }
        Meld34Kind::Daiminkan | Meld34Kind::Ankan | Meld34Kind::Kakan => {
            if let Some(t) = m.tiles.first() {
                for k in 0..4 {
                    tiles_136.push(t.idx() * 4 + k);
                }
            }
        }
    }
    let opened = !matches!(m.kind, Meld34Kind::Ankan);
    let called_136 = m.called_tile.map(|t| t.idx() * 4).filter(|_| opened);
    RiMeld::new(
        mt,
        tiles_136,
        opened,
        m.from_who.map(|w| w as i8).unwrap_or(-1),
        called_136,
    )
}

/// Use an ordinary copy for shape conversion; red bonus is supplied separately.
fn ordinary_id(tile: Tile34) -> u8 {
    tile.idx() * 4 + u8::from(matches!(tile.idx(), 4 | 13 | 22))
}

fn build_evaluator(info: &PlayerInfo34) -> HandEvaluator {
    let tiles_136 = info
        .hand
        .iter()
        .enumerate()
        .flat_map(|(idx, &count)| {
            std::iter::repeat_n(ordinary_id(Tile34(idx as u8)), count as usize)
        })
        .collect();
    let melds = info.melds.iter().map(meld_to_riichienv).collect();
    let mut evaluator = HandEvaluator::new(tiles_136, melds);
    // HandEvaluator exposes the red bonus independently of its shape counts.
    // The adapter has already counted every real red copy, including melds.
    evaluator.aka_dora_count = info.aka_count;
    evaluator
}

/// Compute the point expectation across the wait set.
pub fn expectation(info: &PlayerInfo34, waits: &Waits, allow_riichi: bool) -> ScoreEstimate {
    if waits.is_empty() {
        return ScoreEstimate::default();
    }
    let dora_indicators_136: Vec<u8> = info.dora_indicators.iter().map(|t| t.idx() * 4).collect();
    let evaluator = build_evaluator(info);

    let is_open = info
        .melds
        .iter()
        .any(|m| !matches!(m.kind, Meld34Kind::Ankan));
    let round_wind = wind_from_tile(info.bakaze);
    let player_wind = wind_from_tile(info.jikaze);

    let mut sum_dama = 0.0f64;
    let mut sum_riichi = 0.0f64;
    let mut total_w = 0u32;
    let mut yaku_set: std::collections::BTreeSet<u32> = Default::default();
    let mut has_yaku = false;

    for (tile, left) in waits.iter() {
        if left == 0 {
            continue;
        }
        // A 34-space wait carries no colour information. Estimate an ordinary
        // winning copy rather than silently awarding an unseen red bonus.
        let win_tile_136 = ordinary_id(Tile34(tile));

        let mut cond_dama = Conditions {
            round_wind,
            player_wind,
            ..Conditions::default()
        };
        cond_dama.tsumo = false;
        let res_dama = evaluator.calc(
            win_tile_136,
            dora_indicators_136.clone(),
            vec![],
            Some(cond_dama),
        );

        let dama_pt = if res_dama.is_win {
            res_dama.ron_agari as f64
        } else {
            0.0
        };

        let mut riichi_pt = 0.0f64;
        if !is_open && allow_riichi {
            let cond_riichi = Conditions {
                round_wind,
                player_wind,
                tsumo: false,
                riichi: true,
                ..Conditions::default()
            };
            let res_riichi = evaluator.calc(
                win_tile_136,
                dora_indicators_136.clone(),
                vec![],
                Some(cond_riichi),
            );
            if res_riichi.is_win {
                riichi_pt = res_riichi.ron_agari as f64;
                for y in &res_riichi.yaku {
                    yaku_set.insert(*y);
                }
                has_yaku = true;
            }
        }

        if res_dama.is_win {
            for y in &res_dama.yaku {
                yaku_set.insert(*y);
            }
            has_yaku = true;
        }

        let w = left as u32;
        sum_dama += dama_pt * w as f64;
        sum_riichi += riichi_pt * w as f64;
        total_w += w;
    }

    if total_w == 0 {
        return ScoreEstimate::default();
    }

    ScoreEstimate {
        dama_point: sum_dama / total_w as f64,
        riichi_point: sum_riichi / total_w as f64,
        yaku_ids: yaku_set.into_iter().collect(),
        has_yaku,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::hand::PlayerInfo34Builder;
    use crate::analysis::tile::Tile34;
    use crate::analysis::waits::Waits;

    #[test]
    fn riichi_strictly_increases_over_dama_for_closed_hand() {
        // Pinfu shape — both dama and riichi should produce positive ron points,
        // and riichi should pay strictly more (extra han from the riichi yaku).
        // Using 1m1m pair (yakuless) + ryanmen wait on 5p/8p.
        let info = PlayerInfo34Builder::new()
            .add_many(&[
                "1m", "1m", "2m", "3m", "4m", "2p", "3p", "4p", "6p", "7p", "2s", "3s", "4s",
            ])
            .build();
        let mut waits = Waits::new();
        waits.insert(Tile34::from_mjai("5p").unwrap().idx(), 4);
        waits.insert(Tile34::from_mjai("8p").unwrap().idx(), 4);
        let est = expectation(&info, &waits, true);
        assert!(est.dama_point > 0.0, "dama_point={}", est.dama_point);
        assert!(
            est.riichi_point > est.dama_point,
            "riichi {} should beat dama {}",
            est.riichi_point,
            est.dama_point
        );
    }

    #[test]
    fn ordinary_five_wait_has_no_phantom_red_bonus() {
        let info = PlayerInfo34Builder::new()
            .add_many(&[
                "1m", "1m", "2m", "3m", "4m", "2p", "3p", "4p", "6p", "7p", "2s", "3s", "4s",
            ])
            .build();
        let mut waits = Waits::new();
        waits.insert(Tile34::from_mjai("5p").unwrap().idx(), 4);
        assert_eq!(expectation(&info, &waits, false).dama_point, 5800.0);
    }

    #[test]
    fn red_bonus_and_four_fives_are_preserved() {
        let mut info = PlayerInfo34Builder::new().add_many(&["5p"; 4]).build();
        let evaluator = build_evaluator(&info);
        assert_eq!(evaluator.hand.counts[13], 4);
        assert_eq!(evaluator.aka_dora_count, 0);
        info.aka_count = 1;
        assert_eq!(build_evaluator(&info).aka_dora_count, 1);
        info.melds.push(Meld34 {
            kind: Meld34Kind::Pon,
            tiles: vec![Tile34(4)],
            called_tile: Some(Tile34(4)),
            from_who: Some(1),
            aka_count: 1,
        });
        info.aka_count = 2;
        assert_eq!(build_evaluator(&info).aka_dora_count, 2);
    }

    #[test]
    fn empty_waits_zero() {
        let info = PlayerInfo34Builder::new().build();
        let waits = Waits::new();
        let est = expectation(&info, &waits, true);
        assert_eq!(est.dama_point, 0.0);
        assert_eq!(est.riichi_point, 0.0);
    }
}
