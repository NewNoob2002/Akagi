//! Risk engine — combines per-opponent deal-in vectors into a mixed
//! 34-tile risk vector and selects the best defence discard.

pub mod base;
pub mod wall;

use super::agari_rate::compose;
use super::data::point::{
    open_ron_point_with_dora, RON_POINT_DAMA, RON_POINT_RIICHI, RON_POINT_RIICHI_IPPATSU,
};
use super::data::tenpai::rate_3p;
use super::hand::{OpponentInfo, PlayerInfo34};
use super::tenpai_rate;
use super::tile::{Tile34, TILE_COUNT};

pub use base::RiskVec;

/// One opponent's risk view: per-tile deal-in % + estimated tenpai rate.
#[derive(Debug, Clone)]
pub struct OpponentRiskVec {
    pub seat: u8,
    pub tenpai_rate: f64,
    pub risk: RiskVec,
}

/// Compute one opponent's risk vector with all corrections applied.
///
/// `bakaze` is the round wind, taken from the active player's `PlayerInfo34`.
pub fn for_opponent(active: &PlayerInfo34, op: &OpponentInfo) -> OpponentRiskVec {
    // Genbutsu: opponent's own discards + tiles they let pass after riichi.
    let mut safe = [false; TILE_COUNT];
    for d in &op.discards {
        safe[d.idx() as usize] = true;
    }
    for d in &op.called_from {
        safe[d.idx() as usize] = true;
    }

    // Active dora tiles (next-of-indicator).
    let dora: Vec<Tile34> = active
        .dora_indicators
        .iter()
        .map(|d| d.dora_next())
        .collect();

    let left = active.compute_left_tiles();
    let turns = op.discards.len();

    let mut risk = base::risk_tiles(turns, &safe, &left, &dora, active.bakaze, op.jikaze);

    // Tenpai rate.
    let tedashi = op
        .tedashi
        .iter()
        .skip(op.last_open_meld_discard_count)
        .filter(|&&b| b)
        .count();
    let mr_at = [op.last_open_meld_discard_count];
    let tenpai_rate = if op.is_riichi {
        100.0
    } else {
        let rate_4p = tenpai_rate::estimate(&op.melds, op.discards.len(), &mr_at, tedashi);
        // 3p table doesn't exist; approximate via the saturation curve from
        // mahjong-helper (`rate_4p * (2 - rate_4p / 100)`).
        if active.num_players == 3 {
            rate_3p(rate_4p)
        } else {
            rate_4p
        }
    };

    // Point fix (riichi pumps base, ippatsu more, open uses dora-dependent point).
    let dora_count_in_open: u32 = op
        .melds
        .iter()
        .map(|m| {
            let mut counts = [0u8; TILE_COUNT];
            m.add_to_counts(&mut counts);
            dora.iter()
                .map(|d| counts[d.idx() as usize] as u32)
                .sum::<u32>()
                + m.aka_count as u32
        })
        .sum();
    let point = if op.is_riichi {
        if op.can_ippatsu {
            RON_POINT_RIICHI_IPPATSU
        } else {
            RON_POINT_RIICHI
        }
    } else if !op.melds.is_empty() {
        open_ron_point_with_dora(dora_count_in_open)
    } else {
        RON_POINT_DAMA
    };
    base::fix_with_point(&mut risk, point);

    // Early-outside (only the first 5 discards).
    let early: Vec<Tile34> = op.discards.iter().take(5).copied().collect();
    base::fix_with_early_outside(&mut risk, &early);

    OpponentRiskVec {
        seat: op.seat,
        tenpai_rate,
        risk,
    }
}

/// Combine multiple opponent risk vectors into a single mixed vector.
/// Mirrors Go's `mixedRiskTable`: skips opponents below a 15% tenpai rate
/// threshold; combines remaining contributions via parallel-OR
/// (`a + b - a*b/100`).
pub fn mixed_risk(opponents: &[OpponentRiskVec]) -> RiskVec {
    let mut mixed = [0.0f64; TILE_COUNT];
    for op in opponents {
        if op.tenpai_rate <= 15.0 {
            continue;
        }
        for (m, r) in mixed.iter_mut().zip(op.risk.iter()) {
            let weighted = r * op.tenpai_rate / 100.0;
            *m = compose(*m, weighted);
        }
    }
    mixed
}

/// Pick the safest discard from the player's hand based on the mixed risk.
pub fn best_defence(hand_counts: &super::hand::Counts34, mixed: &RiskVec) -> Option<u8> {
    let mut best_idx: Option<u8> = None;
    let mut best_risk = f64::INFINITY;
    for (i, c) in hand_counts.iter().enumerate() {
        if *c == 0 {
            continue;
        }
        if mixed[i] < best_risk {
            best_risk = mixed[i];
            best_idx = Some(i as u8);
        }
    }
    best_idx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::hand::{OpponentInfo, PlayerInfo34Builder};

    fn baseline_active() -> PlayerInfo34 {
        PlayerInfo34Builder::new()
            .add_many(&[
                "1m", "2m", "3m", "4m", "5m", "6m", "7m", "8m", "9m", "1p", "2p", "3p", "1s",
            ])
            .build()
    }

    #[test]
    fn no_riichi_no_melds_low_tenpai_excluded_from_mixed() {
        let active = baseline_active();
        let op = OpponentInfo {
            seat: 1,
            discards: vec![],
            tedashi: vec![],
            melds: vec![],
            last_open_meld_discard_count: 0,
            is_riichi: false,
            riichi_turn: None,
            can_ippatsu: false,
            jikaze: Tile34::from_mjai("S").unwrap(),
            called_from: vec![],
        };
        let r = for_opponent(&active, &op);
        assert_eq!(r.tenpai_rate, 0.0);
        let m = mixed_risk(&[r]);
        // 0% tenpai-rate opponent contributes nothing to mixed.
        assert!(m.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn riichi_dominates_mixed() {
        let active = baseline_active();
        let op = OpponentInfo {
            seat: 1,
            discards: vec![Tile34::from_mjai("E").unwrap(); 9],
            tedashi: vec![true; 9],
            melds: vec![],
            last_open_meld_discard_count: 0,
            is_riichi: true,
            riichi_turn: Some(8),
            can_ippatsu: false,
            jikaze: Tile34::from_mjai("S").unwrap(),
            called_from: vec![],
        };
        let r = for_opponent(&active, &op);
        assert_eq!(r.tenpai_rate, 100.0);
        // E is genbutsu → safe.
        assert_eq!(r.risk[Tile34::from_mjai("E").unwrap().idx() as usize], 0.0);
        let m = mixed_risk(&[r]);
        // Some non-zero risk should appear on a non-genbutsu suit tile.
        assert!(m[4] > 0.0); // 5m
    }

    #[test]
    fn tenpai_uses_only_hand_discards_after_the_last_call() {
        let tile = Tile34::from_mjai("E").unwrap();
        let mut op = OpponentInfo {
            discards: ["1m", "2m", "3m", "4m", "9m"]
                .map(|s| Tile34::from_mjai(s).unwrap())
                .to_vec(),
            tedashi: vec![true; 5],
            melds: vec![super::super::hand::Meld34 {
                kind: super::super::hand::Meld34Kind::Pon,
                tiles: vec![tile],
                called_tile: Some(tile),
                from_who: Some(2),
                aka_count: 0,
            }],
            last_open_meld_discard_count: 5,
            ..Default::default()
        };
        assert_eq!(for_opponent(&baseline_active(), &op).tenpai_rate, 8.56);
        op.last_open_meld_discard_count = 0;
        assert_eq!(for_opponent(&baseline_active(), &op).tenpai_rate, 31.25);
    }

    #[test]
    fn meld_dora_counts_every_copy_and_ippatsu_increases_risk() {
        use crate::analysis::hand::{Meld34, Meld34Kind};
        let mut active = baseline_active();
        active.dora_indicators = vec![Tile34::from_mjai("4p").unwrap()];
        let mut op = OpponentInfo {
            melds: vec![Meld34 {
                kind: Meld34Kind::Pon,
                tiles: vec![Tile34(13)],
                called_tile: Some(Tile34(13)),
                from_who: Some(2),
                aka_count: 1,
            }],
            ..Default::default()
        };
        let actual = for_opponent(&active, &op).risk;
        let dora = [Tile34(13)];
        let mut expected = base::risk_tiles(
            0,
            &[false; TILE_COUNT],
            &active.compute_left_tiles(),
            &dora,
            active.bakaze,
            op.jikaze,
        );
        base::fix_with_point(&mut expected, open_ron_point_with_dora(4));
        assert_eq!(actual, expected);
        op.is_riichi = true;
        let before = for_opponent(&active, &op).risk;
        op.can_ippatsu = true;
        let after = for_opponent(&active, &op).risk;
        assert!(after.iter().zip(before).any(|(a, b)| *a > b));
    }

    #[test]
    fn best_defence_picks_lowest_risk_in_hand() {
        let active = baseline_active();
        // Mock a mixed vector where 3m is genbutsu but 1m and 2m carry risk.
        let mut mixed = [0.0f64; TILE_COUNT];
        mixed[0] = 5.0;
        mixed[1] = 4.0;
        mixed[2] = 0.0; // 3m genbutsu
        mixed[3] = 8.0; // 4m

        let pick = best_defence(&active.hand, &mixed).unwrap();
        assert_eq!(pick, 2); // 3m chosen
    }
}
