#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::doc_markdown,
    clippy::type_complexity
)]
use super::*;
use crate::poker::{Card, CardNumber, CardSuit};

fn setup_game(
    players: Vec<(u32, &str, u32, PlayerStatus, Option<(Card, Card)>, u32)>,
    community: Vec<Card>,
) -> GameState {
    let mut gs = GameState::new();
    gs.phase = GamePhase::Showdown;
    for (id, name, chips, status, hole_cards, contribution) in players {
        gs.players.insert(
            id,
            Player {
                id,
                name: name.to_string(),
                chips,
                status,
                hole_cards,
                current_bet: 0,
                sitting_out: false,
            },
        );
        gs.player_order.push(id);
        if contribution > 0 {
            gs.pot_contributions.insert(id, contribution);
            gs.pot += contribution;
        }
    }
    gs.community_cards = community;
    gs
}

/// Board: T♠ J♠ Q♠ K♠ 2♦ — enables Royal Flush with A♠, Straight Flush with 9♠, Straight with any Ace.
fn standard_board() -> Vec<Card> {
    vec![
        Card(CardNumber::Ten, CardSuit::Spades),
        Card(CardNumber::Jack, CardSuit::Spades),
        Card(CardNumber::Queen, CardSuit::Spades),
        Card(CardNumber::King, CardSuit::Spades),
        Card(CardNumber::Two, CardSuit::Diamonds),
    ]
}

/// Board: A♠ K♠ Q♠ J♠ T♠ — Royal Flush on board, every player ties.
fn tie_board() -> Vec<Card> {
    vec![
        Card(CardNumber::Ace, CardSuit::Spades),
        Card(CardNumber::King, CardSuit::Spades),
        Card(CardNumber::Queen, CardSuit::Spades),
        Card(CardNumber::Jack, CardSuit::Spades),
        Card(CardNumber::Ten, CardSuit::Spades),
    ]
}

/// With standard_board → Royal Flush (A♠ K♠ Q♠ J♠ T♠).
fn royal_flush_hand() -> (Card, Card) {
    (
        Card(CardNumber::Ace, CardSuit::Spades),
        Card(CardNumber::Three, CardSuit::Clubs),
    )
}

/// With standard_board → Straight Flush K-high (K♠ Q♠ J♠ T♠ 9♠).
fn straight_flush_hand() -> (Card, Card) {
    (
        Card(CardNumber::Nine, CardSuit::Spades),
        Card(CardNumber::Three, CardSuit::Hearts),
    )
}

/// With standard_board → Ace-high Straight (A K Q J T, not flush).
fn straight_hand() -> (Card, Card) {
    (
        Card(CardNumber::Ace, CardSuit::Hearts),
        Card(CardNumber::Three, CardSuit::Hearts),
    )
}

/// With standard_board → High card (K Q J T 6).
fn low_hand() -> (Card, Card) {
    (
        Card(CardNumber::Four, CardSuit::Hearts),
        Card(CardNumber::Six, CardSuit::Clubs),
    )
}

/// With standard_board → High card (K Q J T 8), slightly better than low_hand.
fn low_hand2() -> (Card, Card) {
    (
        Card(CardNumber::Seven, CardSuit::Hearts),
        Card(CardNumber::Eight, CardSuit::Clubs),
    )
}

/// Total amount a player won in the resolved hand, read from `GameState`.
fn player_winnings(gs: &GameState, player_id: u32) -> u32 {
    gs.last_winners
        .iter()
        .filter(|(id, _, _)| *id == player_id)
        .map(|(_, amount, _)| *amount)
        .sum()
}

/// Whether the resolved hand went to showdown (≥2 eligible Active/AllIn
/// players at resolve time). Replaces the old "did a Showdown message
/// appear" check — the engine no longer emits messages, so derive from the
/// setup: a showdown happened iff more than one player had revealed cards.
fn went_to_showdown(gs: &GameState) -> bool {
    gs.player_order
        .iter()
        .filter(|&&id| {
            gs.players.get(&id).is_some_and(|p| {
                matches!(p.status, PlayerStatus::Active | PlayerStatus::AllIn)
                    && p.hole_cards.is_some()
            })
        })
        .count()
        > 1
}

// -----------------------------------------------------------------------
// Test 1: No side pot — single winner
// -----------------------------------------------------------------------
#[test]
fn test_no_side_pot_single_winner() {
    let mut gs = setup_game(
        vec![
            (
                1,
                "Alice",
                0,
                PlayerStatus::Active,
                Some(royal_flush_hand()),
                100,
            ),
            (2, "Bob", 0, PlayerStatus::Active, Some(low_hand()), 100),
            (
                3,
                "Charlie",
                0,
                PlayerStatus::Active,
                Some(low_hand2()),
                100,
            ),
        ],
        standard_board(),
    );
    assert_eq!(gs.pot, 300);

    gs.resolve_hand();

    assert!(went_to_showdown(&gs));
    assert_eq!(player_winnings(&gs, 1), 300);
    assert_eq!(gs.players.get(&1).unwrap().chips, 300);
    assert_eq!(gs.players.get(&2).unwrap().chips, 0);
    assert_eq!(gs.players.get(&3).unwrap().chips, 0);
    assert_eq!(gs.pot, 0);
}

// -----------------------------------------------------------------------
// Test 2: Short-stack all-in wins the main pot
// -----------------------------------------------------------------------
#[test]
fn test_short_stack_allin_wins_main_pot() {
    // A(AllIn,50) best hand, B(Active,100) second-best, C(Active,100) worst.
    // Main pot  = 50×3 = 150 → A
    // Side pot  = 50×2 = 100 → B
    let mut gs = setup_game(
        vec![
            (
                1,
                "Alice",
                0,
                PlayerStatus::AllIn,
                Some(royal_flush_hand()),
                50,
            ),
            (
                2,
                "Bob",
                0,
                PlayerStatus::Active,
                Some(straight_flush_hand()),
                100,
            ),
            (3, "Charlie", 0, PlayerStatus::Active, Some(low_hand()), 100),
        ],
        standard_board(),
    );
    assert_eq!(gs.pot, 250);

    gs.resolve_hand();

    assert!(went_to_showdown(&gs));
    assert_eq!(player_winnings(&gs, 1), 150);
    assert_eq!(player_winnings(&gs, 2), 100);
    assert_eq!(gs.players.get(&1).unwrap().chips, 150);
    assert_eq!(gs.players.get(&2).unwrap().chips, 100);
    assert_eq!(gs.players.get(&3).unwrap().chips, 0);
    assert_eq!(gs.pot, 0);
}

// -----------------------------------------------------------------------
// Test 3: Active player wins everything (main + side)
// -----------------------------------------------------------------------
#[test]
fn test_active_player_wins_all_pots() {
    // B has the best hand → sweeps both pots for 250.
    let mut gs = setup_game(
        vec![
            (1, "Alice", 0, PlayerStatus::AllIn, Some(low_hand()), 50),
            (
                2,
                "Bob",
                0,
                PlayerStatus::Active,
                Some(royal_flush_hand()),
                100,
            ),
            (
                3,
                "Charlie",
                0,
                PlayerStatus::Active,
                Some(low_hand2()),
                100,
            ),
        ],
        standard_board(),
    );
    assert_eq!(gs.pot, 250);

    gs.resolve_hand();

    assert!(went_to_showdown(&gs));
    assert_eq!(player_winnings(&gs, 2), 250);
    assert_eq!(gs.players.get(&1).unwrap().chips, 0);
    assert_eq!(gs.players.get(&2).unwrap().chips, 250);
    assert_eq!(gs.players.get(&3).unwrap().chips, 0);
    assert_eq!(gs.pot, 0);
}

// -----------------------------------------------------------------------
// Test 4: Two all-ins at different levels → three tiers
// -----------------------------------------------------------------------
#[test]
fn test_two_allin_different_levels() {
    // A(AllIn,25) Royal Flush, B(AllIn,50) SF K-high,
    // C(Active,100) Ace-high straight, D(Active,100) high card.
    // Tier 1: 25×4 = 100 → A   (best overall)
    // Tier 2: 25×3 =  75 → B   (best among B,C,D)
    // Tier 3: 50×2 = 100 → C   (best among C,D)
    let mut gs = setup_game(
        vec![
            (
                1,
                "Alice",
                0,
                PlayerStatus::AllIn,
                Some(royal_flush_hand()),
                25,
            ),
            (
                2,
                "Bob",
                0,
                PlayerStatus::AllIn,
                Some(straight_flush_hand()),
                50,
            ),
            (
                3,
                "Charlie",
                0,
                PlayerStatus::Active,
                Some(straight_hand()),
                100,
            ),
            (4, "Diana", 0, PlayerStatus::Active, Some(low_hand()), 100),
        ],
        standard_board(),
    );
    assert_eq!(gs.pot, 275);

    gs.resolve_hand();

    assert!(went_to_showdown(&gs));
    assert_eq!(player_winnings(&gs, 1), 100);
    assert_eq!(player_winnings(&gs, 2), 75);
    assert_eq!(player_winnings(&gs, 3), 100);
    assert_eq!(gs.players.get(&1).unwrap().chips, 100);
    assert_eq!(gs.players.get(&2).unwrap().chips, 75);
    assert_eq!(gs.players.get(&3).unwrap().chips, 100);
    assert_eq!(gs.players.get(&4).unwrap().chips, 0);
    assert_eq!(gs.pot, 0);
}

// -----------------------------------------------------------------------
// Test 5: Folded player's money stays in pot, not eligible to win
// -----------------------------------------------------------------------
#[test]
fn test_folded_player_money_in_pot() {
    // A(Folded,100), B(Active,100) best hand, C(Active,100) worst.
    // Single tier pot = 300. A ineligible → B wins all 300.
    let mut gs = setup_game(
        vec![
            (1, "Alice", 0, PlayerStatus::Folded, None, 100),
            (
                2,
                "Bob",
                0,
                PlayerStatus::Active,
                Some(royal_flush_hand()),
                100,
            ),
            (3, "Charlie", 0, PlayerStatus::Active, Some(low_hand()), 100),
        ],
        standard_board(),
    );
    assert_eq!(gs.pot, 300);

    gs.resolve_hand();

    assert!(went_to_showdown(&gs));
    assert_eq!(player_winnings(&gs, 2), 300);
    assert_eq!(gs.players.get(&1).unwrap().chips, 0);
    assert_eq!(gs.players.get(&2).unwrap().chips, 300);
    assert_eq!(gs.players.get(&3).unwrap().chips, 0);
    assert_eq!(gs.pot, 0);
}

// -----------------------------------------------------------------------
// Test 6: Dead money from multiple folded players → solo active wins all
// -----------------------------------------------------------------------
#[test]
fn test_dead_money_from_folded_players() {
    // A(Folded,30), B(Folded,30), C(Active,100). Only C eligible.
    // Pot = 160. C is the sole active player → solo-survivor path.
    let mut gs = setup_game(
        vec![
            (1, "Alice", 0, PlayerStatus::Folded, None, 30),
            (2, "Bob", 0, PlayerStatus::Folded, None, 30),
            (
                3,
                "Charlie",
                0,
                PlayerStatus::Active,
                Some(royal_flush_hand()),
                100,
            ),
        ],
        standard_board(),
    );
    assert_eq!(gs.pot, 160);

    gs.resolve_hand();

    assert!(
        !went_to_showdown(&gs),
        "solo active player should not trigger showdown"
    );
    assert_eq!(player_winnings(&gs, 3), 160);
    assert_eq!(gs.players.get(&3).unwrap().chips, 160);
    assert_eq!(gs.pot, 0);
}

// -----------------------------------------------------------------------
// Test 7: Split pot — perfect tie (Royal Flush on board)
// -----------------------------------------------------------------------
#[test]
fn test_split_pot_tie() {
    let mut gs = setup_game(
        vec![
            (1, "Alice", 0, PlayerStatus::Active, Some(low_hand()), 100),
            (2, "Bob", 0, PlayerStatus::Active, Some(low_hand2()), 100),
        ],
        tie_board(),
    );
    assert_eq!(gs.pot, 200);

    gs.resolve_hand();

    assert!(went_to_showdown(&gs));
    assert_eq!(player_winnings(&gs, 1), 100);
    assert_eq!(player_winnings(&gs, 2), 100);
    assert_eq!(gs.players.get(&1).unwrap().chips, 100);
    assert_eq!(gs.players.get(&2).unwrap().chips, 100);
    assert_eq!(gs.pot, 0);
}

// -----------------------------------------------------------------------
// Test 8: Odd chip in split → lower player_id gets the extra chip
// -----------------------------------------------------------------------
#[test]
fn test_odd_chip_split() {
    // P1(Active,100) and P2(Active,100) tie on tie_board.
    // P3(Folded,1) adds 1 dead chip → pot = 201.
    // Tier 1 (level 1): 1×3 = 3, eligible P1 & P2, tie → P1 gets 2, P2 gets 1.
    // Tier 2 (level 100): 99×2 = 198, eligible P1 & P2, tie → 99 each.
    // Totals: P1 = 101, P2 = 100.
    let mut gs = setup_game(
        vec![
            (1, "Alice", 0, PlayerStatus::Active, Some(low_hand()), 100),
            (2, "Bob", 0, PlayerStatus::Active, Some(low_hand2()), 100),
            (3, "Charlie", 0, PlayerStatus::Folded, None, 1),
        ],
        tie_board(),
    );
    assert_eq!(gs.pot, 201);

    gs.resolve_hand();

    let p1_chips = gs.players.get(&1).unwrap().chips;
    let p2_chips = gs.players.get(&2).unwrap().chips;
    assert_eq!(p1_chips + p2_chips, 201, "all chips must be distributed");
    assert!(
        p1_chips >= p2_chips,
        "lower player_id should get the odd chip"
    );
    assert_eq!(p1_chips, 101);
    assert_eq!(p2_chips, 100);
    assert_eq!(gs.pot, 0);
}

// -----------------------------------------------------------------------
// Test 9: Solo survivor — everyone else folded, no showdown
// -----------------------------------------------------------------------
#[test]
fn test_solo_survivor_everyone_folded() {
    let mut gs = setup_game(
        vec![
            (
                1,
                "Alice",
                0,
                PlayerStatus::Active,
                Some(royal_flush_hand()),
                100,
            ),
            (2, "Bob", 0, PlayerStatus::Folded, None, 100),
            (3, "Charlie", 0, PlayerStatus::Folded, None, 50),
        ],
        standard_board(),
    );
    assert_eq!(gs.pot, 250);

    gs.resolve_hand();

    assert!(
        !went_to_showdown(&gs),
        "solo survivor should not trigger showdown"
    );
    assert_eq!(player_winnings(&gs, 1), 250);
    assert_eq!(gs.players.get(&1).unwrap().chips, 250);
    assert_eq!(gs.pot, 0);
    // Won by fold (no showdown): no rank is recorded.
    assert_eq!(gs.last_winners, vec![(1, 250, None)]);
}

// -----------------------------------------------------------------------
// Test 10: All-in wins main pot, different active player wins side pot
// -----------------------------------------------------------------------
#[test]
fn test_allin_wins_main_active_wins_side() {
    // A(AllIn,50) Royal Flush, B(Active,100) Ace-high straight, C(Active,100) high card.
    // Main pot = 50×3 = 150 → A
    // Side pot = 50×2 = 100 → B
    let mut gs = setup_game(
        vec![
            (
                1,
                "Alice",
                0,
                PlayerStatus::AllIn,
                Some(royal_flush_hand()),
                50,
            ),
            (
                2,
                "Bob",
                0,
                PlayerStatus::Active,
                Some(straight_hand()),
                100,
            ),
            (3, "Charlie", 0, PlayerStatus::Active, Some(low_hand()), 100),
        ],
        standard_board(),
    );
    assert_eq!(gs.pot, 250);

    gs.resolve_hand();

    assert!(went_to_showdown(&gs));
    assert_eq!(player_winnings(&gs, 1), 150);
    assert_eq!(player_winnings(&gs, 2), 100);
    assert_eq!(gs.players.get(&1).unwrap().chips, 150);
    assert_eq!(gs.players.get(&2).unwrap().chips, 100);
    assert_eq!(gs.players.get(&3).unwrap().chips, 0);
    assert_eq!(gs.pot, 0);
}

// -----------------------------------------------------------------------
// Test 11: Multi-player hand must NOT end the game (regression)
// -----------------------------------------------------------------------
#[test]
fn test_resolve_hand_does_not_end_game_with_multiple_survivors() {
    // Three players still hold chips. Resolving the pot is just "hand
    // over", not "game over" — the engine must keep the session alive so
    // the next hand can be dealt. Regression for the bug where
    // `remaining.first()` being `Some` (true after any hand) was taken as
    // the game-over signal, which tore the game down mid-match.
    let mut gs = setup_game(
        vec![
            (
                1,
                "Alice",
                200,
                PlayerStatus::Active,
                Some(royal_flush_hand()),
                100,
            ),
            (2, "Bob", 100, PlayerStatus::Active, Some(low_hand()), 100),
            (3, "Charlie", 50, PlayerStatus::Folded, None, 0),
        ],
        standard_board(),
    );
    assert_eq!(gs.pot, 200);
    // Simulate an in-progress game so is_game_over's hand_number guard is met.
    gs.game_started = true;
    gs.hand_number = 5;

    gs.resolve_hand();

    // Alice wins the pot; Bob and Charlie still hold chips → not game over.
    assert!(
        !gs.is_game_over(),
        "GameOver must not fire while more than one player has chips"
    );
    assert!(gs.game_started, "game_started must stay true mid-game");
}

// -----------------------------------------------------------------------
// Test 12: Game ends only when exactly one player has chips
// -----------------------------------------------------------------------
#[test]
fn test_resolve_hand_ends_game_with_single_survivor() {
    // Everyone except one player is at zero chips — genuine game over.
    let mut gs = setup_game(
        vec![
            (
                1,
                "Alice",
                0,
                PlayerStatus::Active,
                Some(royal_flush_hand()),
                100,
            ),
            (2, "Bob", 0, PlayerStatus::Active, Some(low_hand()), 0),
        ],
        standard_board(),
    );
    assert_eq!(gs.pot, 100);
    gs.game_started = true;
    gs.hand_number = 5;

    gs.resolve_hand();

    assert!(
        gs.is_game_over(),
        "GameOver should fire when exactly one player has chips"
    );
    // The sole survivor holding chips is the winner.
    let winner = gs
        .players
        .values()
        .find(|p| p.chips > 0)
        .expect("one player should hold all chips");
    assert_eq!(winner.id, 1);
    assert!(
        !gs.game_started,
        "game_started must be cleared on game over"
    );
}

/// Blinds don't increase on a fresh `start_new_hand` (no anchor set yet —
/// the anchor is established on game start, not on construction).
#[test]
fn test_blinds_no_increase_without_anchor() {
    let mut gs = GameState::new();
    gs.blind_config = BlindConfig {
        interval_secs: 1,
        increase_percent: 50,
    };
    let before = gs.big_blind;
    // last_blind_increase is None → no increase possible.
    gs.start_new_hand();
    assert_eq!(gs.big_blind, before);
}

/// Blinds don't increase before the interval has elapsed.
#[test]
fn test_blinds_no_increase_within_interval() {
    let mut gs = GameState::new();
    gs.blind_config = BlindConfig {
        interval_secs: 60,
        increase_percent: 50,
    };
    gs.last_blind_increase = Some(Instant::now());
    let before = gs.big_blind;
    gs.start_new_hand();
    assert_eq!(gs.big_blind, before);
}

/// One interval elapsed → exactly one increase.
#[test]
fn test_blinds_single_increase_after_one_interval() {
    let mut gs = GameState::new();
    gs.blind_config = BlindConfig {
        interval_secs: 1,
        increase_percent: 50,
    };
    // Anchor 1s in the past so exactly one interval has elapsed.
    gs.last_blind_increase = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .map(Some)
        .unwrap();
    gs.start_new_hand();
    // 20 + ceil(20*50/100) = 30.
    assert_eq!(gs.big_blind, 30);
}

/// A gap spanning three intervals must apply THREE increases (catch-up),
/// not one — the anchor advances by one interval per step rather than
/// resetting to `Instant::now()`.
#[test]
fn test_blinds_catch_up_multiple_missed_levels() {
    let mut gs = GameState::new();
    gs.blind_config = BlindConfig {
        interval_secs: 1,
        increase_percent: 100, // doubles each step for easy math
    };
    // Anchor 3s in the past: three intervals have elapsed.
    gs.last_blind_increase = Instant::now()
        .checked_sub(Duration::from_secs(3))
        .map(Some)
        .unwrap();
    gs.start_new_hand();
    // 20 → 40 → 80 → 160.
    assert_eq!(
        gs.big_blind, 160,
        "a 3-interval gap must catch up all three levels"
    );
}

/// After catch-up the anchor sits on the interval boundary (not "now"), so
/// one more interval steps exactly once more.
#[test]
fn test_blinds_anchor_advances_by_interval_not_now() {
    let mut gs = GameState::new();
    gs.blind_config = BlindConfig {
        interval_secs: 1,
        increase_percent: 100,
    };
    // Anchor 2s in the past.
    gs.last_blind_increase = Instant::now()
        .checked_sub(Duration::from_secs(2))
        .map(Some)
        .unwrap();
    // First hand: catches up two levels → 20 → 40 → 80. Anchor now at
    // (original + 2s), i.e. ~now.
    gs.start_new_hand();
    assert_eq!(gs.big_blind, 80);

    // Simulate one more interval passing before the next hand.
    gs.last_blind_increase = gs
        .last_blind_increase
        .and_then(|t| t.checked_sub(Duration::from_secs(1)));
    gs.start_new_hand();
    assert_eq!(
        gs.big_blind, 160,
        "exactly one step after one more interval"
    );
}

/// Host removed → host rights go to the lowest-id remaining player.
#[test]
fn test_promote_next_host_promotes_lowest_remaining() {
    let mut gs = GameState::new();
    let host = gs.add_player("host".into()).id; // id 1
    let p2 = gs.add_player("two".into()).id; // id 2
    let p3 = gs.add_player("three".into()).id; // id 3
    gs.host_id = host;

    gs.remove_player(host);
    let promoted = gs.promote_next_host(host);
    assert_eq!(promoted, Some(p2));
    assert_eq!(gs.host_id, p2);
    // p3 remains present.
    assert!(gs.players.contains_key(&p3));
}

/// Removing a non-host player leaves `host_id` untouched.
#[test]
fn test_promote_next_host_noop_for_non_host() {
    let mut gs = GameState::new();
    let host = gs.add_player("host".into()).id;
    let other = gs.add_player("other".into()).id;
    gs.host_id = host;

    gs.remove_player(other);
    assert_eq!(gs.promote_next_host(other), None);
    assert_eq!(gs.host_id, host);
}

/// Last player removed → no one to promote, returns `None`.
#[test]
fn test_promote_next_host_none_when_empty() {
    let mut gs = GameState::new();
    let host = gs.add_player("host".into()).id;
    gs.host_id = host;

    gs.remove_player(host);
    assert_eq!(gs.promote_next_host(host), None);
    assert!(gs.player_order.is_empty());
}

// -----------------------------------------------------------------------
// Test 13: A resolved hand lands in HandOver (not stuck on River) and
// records its winners; the next deal clears both.
// -----------------------------------------------------------------------
#[test]
fn test_resolve_hand_lands_in_handover_with_winners() {
    // Three players still in (genuine showdown), two will hold chips after
    // the pot is awarded → not game over. Pre-resolve, simulate a river
    // check-down by starting from the Showdown-ish setup_game helper.
    let mut gs = setup_game(
        vec![
            (
                1,
                "Alice",
                200,
                PlayerStatus::Active,
                Some(royal_flush_hand()),
                100,
            ),
            (2, "Bob", 100, PlayerStatus::Active, Some(low_hand()), 100),
            (3, "Charlie", 50, PlayerStatus::Folded, None, 0),
        ],
        standard_board(),
    );
    gs.game_started = true;
    gs.hand_number = 5;
    gs.phase = GamePhase::River;

    gs.resolve_hand();

    // Phase must move off River to HandOver — staying on River is the bug
    // that lit up a phantom turn/timer/action bar during the pre-deal wait.
    assert_eq!(
        gs.phase,
        GamePhase::HandOver,
        "phase must be HandOver after resolve"
    );
    assert!(
        !gs.is_game_over(),
        "not game over — two players still hold chips"
    );
    // Alice (royal flush) wins the 200 pot.
    assert_eq!(gs.last_winners.len(), 1);
    let (wid, amount, rank) = gs.last_winners.first().expect("winner recorded");
    assert_eq!(*wid, 1);
    assert_eq!(*amount, 200);
    // Showdown win: the winning hand's rank is recorded.
    assert_eq!(*rank, Some(HandRank::RoyalFlush));
}

// -----------------------------------------------------------------------
// apply_action tests
// -----------------------------------------------------------------------

/// Set up a 2-player PreFlop game with `current_player_index` pointing at
/// `actor_id`. Both players Active with plenty of chips.
fn heads_up_preflop(actor_id: u32) -> GameState {
    let mut gs = GameState::new();
    gs.big_blind = 20;
    gs.small_blind = 10;
    gs.game_started = true;
    gs.phase = GamePhase::PreFlop;
    for id in [1, 2] {
        gs.players.insert(
            id,
            Player {
                id,
                name: format!("p{id}"),
                chips: 1000,
                status: PlayerStatus::Active,
                hole_cards: None,
                current_bet: 0,
                sitting_out: false,
            },
        );
        gs.player_order.push(id);
    }
    // Player 1 = dealer/SB, player 2 = BB. Action starts after the BB,
    // i.e. back on player 1, so make the actor the SB unless overridden.
    let idx = gs
        .player_order
        .iter()
        .position(|&id| id == actor_id)
        .expect("actor must be seated");
    gs.current_player_index = idx;
    gs.current_bet = gs.big_blind;
    gs.min_raise = gs.big_blind;
    gs.has_acted_this_round = false;
    gs
}

#[test]
fn test_apply_action_fold() {
    let mut gs = heads_up_preflop(1);
    gs.apply_action(1, PlayerAction::Fold, 0).unwrap();
    assert_eq!(gs.players.get(&1).unwrap().status, PlayerStatus::Folded);
    // Turn advanced to the other active player.
    assert_eq!(gs.current_player_id(), Some(2));
}

#[test]
fn test_apply_action_rejects_wrong_player() {
    let mut gs = heads_up_preflop(1);
    assert_eq!(
        gs.apply_action(2, PlayerAction::Check, 0),
        Err(ActionError::NotYourTurn)
    );
}

#[test]
fn test_apply_action_rejects_invalid_action() {
    let mut gs = heads_up_preflop(1);
    // Raise with 0 amount is below the min raise floor and not an all-in.
    assert_eq!(
        gs.apply_action(1, PlayerAction::Raise, 0),
        Err(ActionError::RaiseBelowMinimum { min: 20 })
    );
}

#[test]
fn test_apply_action_call_moves_chips_to_pot() {
    let mut gs = heads_up_preflop(1);
    // SB calls the BB: posts the remaining 10 to match the 20 BB.
    gs.players.get_mut(&1).unwrap().current_bet = 10;
    let before = gs.players.get(&1).unwrap().chips;
    gs.apply_action(1, PlayerAction::Call, 0).unwrap();
    assert_eq!(gs.players.get(&1).unwrap().chips, before - 10);
    assert_eq!(gs.pot, 10);
    assert_eq!(*gs.pot_contributions.get(&1).unwrap(), 10);
    assert_eq!(gs.players.get(&1).unwrap().current_bet, 20);
}

#[test]
fn test_apply_action_call_allin_sets_status() {
    let mut gs = heads_up_preflop(1);
    // Give the actor exactly the to_call amount so the call empties them.
    let to_call = gs
        .current_bet
        .saturating_sub(gs.players.get(&1).unwrap().current_bet);
    gs.players.get_mut(&1).unwrap().chips = to_call;
    gs.apply_action(1, PlayerAction::Call, 0).unwrap();
    assert_eq!(gs.players.get(&1).unwrap().chips, 0);
    assert_eq!(gs.players.get(&1).unwrap().status, PlayerStatus::AllIn);
}

#[test]
fn test_apply_action_raise_reopens_betting() {
    let mut gs = heads_up_preflop(1);
    // Raise by 60 over a 20 current bet → new current_bet 80, increment 60
    // ≥ min_raise (20), so last_raiser_index is set and min_raise bumps.
    // Capture the actor's index before the action: apply_action sets
    // last_raiser_index to the *raisers* index, then advances the turn.
    let raiser_index = gs.current_player_index;
    gs.apply_action(1, PlayerAction::Raise, 60).unwrap();
    assert_eq!(gs.current_bet, 80);
    assert_eq!(gs.min_raise, 60);
    assert_eq!(gs.last_raiser_index, Some(raiser_index));
}

#[test]
fn test_apply_action_raise_below_minimum_allin_is_allowed() {
    let mut gs = heads_up_preflop(1);
    // current_bet 20, player owes 20 (to_call 20), has 25 chips. Raising by
    // 5 → raise_total 25 == chips (all-in), below the 20 floor. Raise is
    // still valid (chips 25 > to_call 20), so the all-in carve-out applies:
    // allowed, and must NOT reopen betting.
    gs.players.get_mut(&1).unwrap().chips = 25;
    gs.apply_action(1, PlayerAction::Raise, 5).unwrap();
    assert_eq!(gs.players.get(&1).unwrap().chips, 0);
    assert_eq!(gs.players.get(&1).unwrap().status, PlayerStatus::AllIn);
    // Sub-min all-in raise: last_raiser_index unchanged from the seed (None).
    assert_eq!(gs.last_raiser_index, None);
}

#[test]
fn test_apply_action_allin_below_current_bet_does_not_reopen() {
    let mut gs = heads_up_preflop(1);
    // Actor owes 20 (to_call 20) but has only 5 chips. All-in for 5 is
    // below current_bet, so current_bet must NOT change and betting must
    // NOT reopen.
    gs.players.get_mut(&1).unwrap().chips = 5;
    gs.apply_action(1, PlayerAction::AllIn, 0).unwrap();
    assert_eq!(gs.players.get(&1).unwrap().chips, 0);
    assert_eq!(gs.current_bet, 20);
    assert_eq!(gs.last_raiser_index, None);
}

#[test]
fn test_apply_action_check_gated_by_valid_actions() {
    let mut gs = heads_up_preflop(1);
    // SB still owes 10 (to_call != 0). valid_actions excludes Check, so
    // apply_action rejects at the validity guard with InvalidAction before
    // reaching the Check arm's defensive CannotCheck guard.
    gs.players.get_mut(&1).unwrap().current_bet = 10;
    assert_eq!(
        gs.apply_action(1, PlayerAction::Check, 0),
        Err(ActionError::InvalidAction)
    );
}

#[test]
fn test_apply_action_game_not_started() {
    let mut gs = heads_up_preflop(1);
    gs.game_started = false;
    assert_eq!(
        gs.apply_action(1, PlayerAction::Fold, 0),
        Err(ActionError::GameNotStarted)
    );
}

#[test]
fn test_auto_action_prefers_check() {
    let mut gs = heads_up_preflop(1);
    // Match the bet so Check is legal.
    gs.players.get_mut(&1).unwrap().current_bet = gs.current_bet;
    assert_eq!(gs.auto_action(1), Some(PlayerAction::Check));
}

#[test]
fn test_auto_action_folds_when_check_unavailable() {
    let gs = heads_up_preflop(1);
    // SB owes the BB → Check not legal, so auto-folds.
    assert_eq!(gs.auto_action(1), Some(PlayerAction::Fold));
}

// -----------------------------------------------------------------------
// Test 14: start_new_hand clears the HandOver phase and last_winners.
// -----------------------------------------------------------------------
#[test]
fn test_start_new_hand_clears_handover() {
    let mut gs = setup_game(
        vec![
            (
                1,
                "Alice",
                200,
                PlayerStatus::Active,
                Some(royal_flush_hand()),
                100,
            ),
            (2, "Bob", 100, PlayerStatus::Active, Some(low_hand()), 100),
        ],
        standard_board(),
    );
    gs.game_started = true;
    gs.hand_number = 5;
    gs.phase = GamePhase::River;

    gs.resolve_hand();
    assert_eq!(gs.phase, GamePhase::HandOver);
    assert!(!gs.last_winners.is_empty());

    gs.start_new_hand();
    assert_eq!(gs.phase, GamePhase::PreFlop, "new deal must clear HandOver");
    assert!(
        gs.last_winners.is_empty(),
        "last_winners must be cleared by the next deal"
    );
}

// -----------------------------------------------------------------------
// dealable_player_count / try_start / apply_settings
// -----------------------------------------------------------------------

/// A fresh lobby with two seated, chip-bearing players has 2 dealable. Sitting
/// out and chipless players don't count toward the deal threshold.
#[test]
fn test_dealable_player_count_excludes_sitting_out_and_chipless() {
    let mut gs = GameState::new();
    // Two healthy players.
    let p1 = gs.add_player("a".into()).id;
    let p2 = gs.add_player("b".into()).id;
    // A third who is sitting out.
    let p3 = gs.add_player("c".into()).id;
    gs.players.get_mut(&p3).unwrap().sitting_out = true;
    // A fourth with no chips.
    let p4 = gs.add_player("d".into()).id;
    gs.players.get_mut(&p4).unwrap().chips = 0;

    assert_eq!(gs.dealable_player_count(), 2);

    gs.set_sitting_out(p1);
    assert_eq!(gs.dealable_player_count(), 1, "sitting-out p1 excluded");

    gs.set_sitting_in(p1);
    assert_eq!(gs.dealable_player_count(), 2);

    // Removing p2 drops to 1 (p3/p4 never counted).
    gs.remove_player(p2);
    assert_eq!(gs.dealable_player_count(), 1);
    // p1 is still dealable; p3/p4 names are referenced to keep them live.
    let _ = (p1, p3, p4);
}

/// try_start validates host / player-count / already-started, and on success
/// freezes the starting baseline and deals the first hand.
#[test]
fn test_try_start_errors_and_happy_path() {
    // --- NotHost: only the host may start. ---
    let mut gs = GameState::new();
    let host = gs.add_player("host".into()).id;
    let _other = gs.add_player("x".into()).id;
    gs.host_id = host;
    assert_eq!(
        gs.try_start(host.saturating_add(1)),
        Err(StartGameError::NotHost)
    );
    assert!(!gs.game_started, "failed start must not flip game_started");

    // --- NotEnoughPlayers: a lone player can't start. ---
    let mut gs = GameState::new();
    let host = gs.add_player("host".into()).id;
    gs.host_id = host;
    assert_eq!(gs.try_start(host), Err(StartGameError::NotEnoughPlayers));

    // --- Happy path: two players, host starts. ---
    let mut gs = GameState::new();
    let host = gs.add_player("host".into()).id;
    let bb = gs.big_blind;
    let _p2 = gs.add_player("two".into()).id;
    gs.host_id = host;
    gs.starting_bbs = 50;

    gs.try_start(host).expect("host with 2 players may start");
    assert!(gs.game_started);
    assert_eq!(gs.phase, GamePhase::PreFlop, "first hand dealt");
    assert_eq!(gs.hand_number, 1);
    // starting_chips = starting_bbs * big_blind, frozen at start.
    assert_eq!(gs.starting_chips, 50 * bb);
    assert_eq!(gs.starting_big_blind, bb);
    // Blinds not configured → no anchor set.
    assert!(gs.last_blind_increase.is_none());

    // --- AlreadyStarted: starting twice is rejected. ---
    assert_eq!(gs.try_start(host), Err(StartGameError::AlreadyStarted));
}

/// try_start seeds the blind-schedule anchor when rising blinds are on.
#[test]
fn test_try_start_seeds_blind_anchor() {
    let mut gs = GameState::new();
    let host = gs.add_player("host".into()).id;
    let _p2 = gs.add_player("two".into()).id;
    gs.host_id = host;
    gs.blind_config = BlindConfig {
        interval_secs: 60,
        increase_percent: 50,
    };
    gs.try_start(host).unwrap();
    assert!(
        gs.last_blind_increase.is_some(),
        "anchor seeded when rising blinds configured"
    );
}

/// apply_settings pre-game rebuys every seated player at the new stack;
/// mid-game it ignores starting_bbs and re-anchors the blind schedule.
#[test]
fn test_apply_settings_pre_game_rebuys_existing_players() {
    let mut gs = GameState::new();
    gs.big_blind = 20;
    gs.starting_bbs = 100;
    let p1 = gs.add_player("a".into()).id;
    let p2 = gs.add_player("b".into()).id;
    // Both seated at the original 100 BB buy-in (100 * 20 = 2000).
    assert_eq!(gs.players.get(&p1).unwrap().chips, 2000);

    // Host raises the stack to 300 BBs pre-game.
    let config = BlindConfig {
        interval_secs: 300,
        increase_percent: 50,
    };
    gs.apply_settings(config, 300);

    assert_eq!(gs.starting_bbs, 300);
    let new_stack = 300 * 20;
    assert_eq!(gs.players.get(&p1).unwrap().chips, new_stack);
    assert_eq!(gs.players.get(&p2).unwrap().chips, new_stack);
    assert_eq!(gs.blind_config.interval_secs, 300);
    assert_eq!(gs.blind_config.increase_percent, 50);
}

/// Mid-game, apply_settings must not touch starting_bbs or chips, but must
/// re-anchor the blind schedule to ~now.
#[test]
fn test_apply_settings_mid_game_ignores_stack_and_reanchors() {
    let mut gs = GameState::new();
    gs.big_blind = 20;
    gs.starting_bbs = 200;
    gs.game_started = true;
    gs.blind_config = BlindConfig {
        interval_secs: 60,
        increase_percent: 50,
    };
    // Anchor an hour in the past.
    gs.last_blind_increase = Some(
        Instant::now()
            .checked_sub(Duration::from_secs(3600))
            .unwrap(),
    );
    let anchor_before = gs.last_blind_increase;
    let p1 = gs.add_player_with_chips("a".into(), Some(1000)).id;
    let chips_before = gs.players.get(&p1).unwrap().chips;

    let config = BlindConfig {
        interval_secs: 300,
        increase_percent: 50,
    };
    gs.apply_settings(config, 999);

    assert_eq!(gs.starting_bbs, 200, "mid-game stack edit ignored");
    assert_eq!(
        gs.players.get(&p1).unwrap().chips,
        chips_before,
        "mid-game chips untouched"
    );
    assert_eq!(gs.blind_config.interval_secs, 300);
    assert!(
        gs.last_blind_increase > anchor_before,
        "blind schedule re-anchored to ~now"
    );
}

/// The phase predicates the transport renders turns and bets from.
#[test]
fn test_phase_predicates() {
    for phase in [
        GamePhase::PreFlop,
        GamePhase::Flop,
        GamePhase::Turn,
        GamePhase::River,
    ] {
        assert!(phase.is_betting(), "{phase:?} has a live turn");
        assert!(phase.is_in_hand(), "{phase:?} has live bets");
    }
    assert!(!GamePhase::Lobby.is_betting());
    assert!(!GamePhase::Showdown.is_betting());
    assert!(!GamePhase::HandOver.is_betting());
    assert!(!GamePhase::Lobby.is_in_hand());
    assert!(GamePhase::Showdown.is_in_hand());
    assert!(!GamePhase::HandOver.is_in_hand());
}

/// The GameState-level phase predicates gate on `game_started` as well.
#[test]
fn test_game_state_phase_predicates() {
    let mut gs = GameState::new();
    let host = gs.add_player("a".into()).id;
    let _p2 = gs.add_player("b".into()).id;
    gs.host_id = host;
    assert!(!gs.is_betting_phase(), "lobby has no turn");
    assert!(!gs.is_in_hand(), "lobby has no live hand");

    gs.try_start(host).unwrap();
    assert!(gs.is_betting_phase(), "preflop has a live turn");
    assert!(gs.is_in_hand(), "preflop has live bets");
}

/// The blind-seat queries expose the same rule `start_new_hand` posts from:
/// small blind one seat clockwise of the button, big blind two seats.
#[test]
fn test_blind_seat_queries_follow_dealer() {
    let mut gs = GameState::new();
    let host = gs.add_player("a".into()).id;
    let _p2 = gs.add_player("b".into()).id;
    let p3 = gs.add_player("c".into()).id;
    gs.host_id = host;
    gs.try_start(host).unwrap();

    // The first deal moves the button to seat 1.
    assert_eq!(gs.dealer_index, 1);
    assert_eq!(gs.small_blind_seat(), Some(2));
    assert_eq!(gs.big_blind_seat(), Some(0));
    // player_order = [1, 2, 3]: seat 2 is player 3, seat 0 is player 1.
    assert_eq!(gs.small_blind_id(), Some(p3));
    assert_eq!(gs.big_blind_id(), Some(host));
    // The engine posted exactly those amounts from exactly those seats.
    assert_eq!(gs.players.get(&p3).unwrap().current_bet, gs.small_blind);
    assert_eq!(gs.players.get(&host).unwrap().current_bet, gs.big_blind);
}

/// Heads-up the same offsets apply: the non-dealer seat posts the small
/// blind and the dealer posts the big blind. (Note: standard heads-up
/// convention assigns them the other way around — dealer posts the small
/// blind. This test pins the engine's current rule.)
#[test]
fn test_blind_seat_queries_heads_up() {
    let mut gs = GameState::new();
    let host = gs.add_player("a".into()).id;
    let p2 = gs.add_player("b".into()).id;
    gs.host_id = host;
    gs.try_start(host).unwrap();

    assert_eq!(gs.dealer_index, 1);
    assert_eq!(gs.small_blind_seat(), Some(0));
    assert_eq!(gs.big_blind_seat(), Some(1));
    assert_eq!(gs.small_blind_id(), Some(host));
    assert_eq!(gs.big_blind_id(), Some(p2));
}
