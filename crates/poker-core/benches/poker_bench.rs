use criterion::{Criterion, criterion_group, criterion_main};
use poker_core::poker::*;
use std::hint::black_box;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_board(flop: Option<[Card; 3]>, turn: Option<Card>, river: Option<Card>) -> Board {
    Board {
        flop: flop.map(|[a, b, c]| (a, b, c)),
        turn,
        river,
    }
}

fn c(rank: CardNumber, suit: CardSuit) -> Card {
    Card(rank, suit)
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

/// Benchmark `Hand::best` — the full 7-card evaluation path.
fn bench_hand_best(crit: &mut Criterion) {
    // A hand that resolves to a flush (exercises several try_* branches)
    let hand = Hand(
        c(CardNumber::Ace, CardSuit::Hearts),
        c(CardNumber::King, CardSuit::Hearts),
    );
    let board = make_board(
        Some([
            c(CardNumber::Ten, CardSuit::Hearts),
            c(CardNumber::Seven, CardSuit::Hearts),
            c(CardNumber::Two, CardSuit::Hearts),
        ]),
        Some(c(CardNumber::Nine, CardSuit::Clubs)),
        Some(c(CardNumber::Three, CardSuit::Diamonds)),
    );

    crit.bench_function("hand_best_flush", |b| {
        b.iter(|| black_box(hand.best(black_box(&board))))
    });
}

/// Benchmark `Hand::best` for a high-card hand (worst case — tries every branch).
fn bench_hand_best_high_card(crit: &mut Criterion) {
    let hand = Hand(
        c(CardNumber::Ace, CardSuit::Spades),
        c(CardNumber::Nine, CardSuit::Hearts),
    );
    let board = make_board(
        Some([
            c(CardNumber::Two, CardSuit::Diamonds),
            c(CardNumber::Five, CardSuit::Clubs),
            c(CardNumber::Seven, CardSuit::Hearts),
        ]),
        Some(c(CardNumber::Jack, CardSuit::Clubs)),
        Some(c(CardNumber::Three, CardSuit::Diamonds)),
    );

    crit.bench_function("hand_best_high_card", |b| {
        b.iter(|| black_box(hand.best(black_box(&board))))
    });
}

/// Benchmark `FullHand::rank` in isolation.
fn bench_full_hand_rank(crit: &mut Criterion) {
    let fh = FullHand(
        c(CardNumber::King, CardSuit::Hearts),
        c(CardNumber::King, CardSuit::Spades),
        c(CardNumber::King, CardSuit::Clubs),
        c(CardNumber::Seven, CardSuit::Diamonds),
        c(CardNumber::Seven, CardSuit::Hearts),
    );

    crit.bench_function("full_hand_rank", |b| {
        b.iter(|| black_box(black_box(&fh).rank()))
    });
}

/// Benchmark `FullHand::compare`.
fn bench_full_hand_compare(crit: &mut Criterion) {
    let h1 = FullHand(
        c(CardNumber::King, CardSuit::Hearts),
        c(CardNumber::King, CardSuit::Spades),
        c(CardNumber::King, CardSuit::Clubs),
        c(CardNumber::Seven, CardSuit::Diamonds),
        c(CardNumber::Seven, CardSuit::Hearts),
    );
    let h2 = FullHand(
        c(CardNumber::Queen, CardSuit::Hearts),
        c(CardNumber::Queen, CardSuit::Spades),
        c(CardNumber::Queen, CardSuit::Clubs),
        c(CardNumber::Jack, CardSuit::Diamonds),
        c(CardNumber::Jack, CardSuit::Hearts),
    );

    crit.bench_function("full_hand_compare", |b| {
        b.iter(|| black_box(black_box(&h1).compare(black_box(&h2))))
    });
}

/// Benchmark `get_all_cards` (allocation cost).
fn bench_get_all_cards(crit: &mut Criterion) {
    crit.bench_function("get_all_cards", |b| b.iter(|| black_box(get_all_cards())));
}

/// Benchmark `get_all_numbers` (allocation cost).
fn bench_get_all_numbers(crit: &mut Criterion) {
    crit.bench_function("get_all_numbers", |b| {
        b.iter(|| black_box(get_all_numbers()))
    });
}

/// Benchmark `calculate_equity` — 1 000 iterations of Monte Carlo.
fn bench_equity_1k(crit: &mut Criterion) {
    let hero = Hand(
        c(CardNumber::Ace, CardSuit::Spades),
        c(CardNumber::King, CardSuit::Spades),
    );
    let board = Board {
        flop: None,
        turn: None,
        river: None,
    };

    crit.bench_function("equity_1k_preflop", |b| {
        b.iter(|| black_box(calculate_equity(black_box(&hero), black_box(&board), 1_000)))
    });
}

/// Benchmark `calculate_equity_multi` — 2 players, 1 000 iterations.
fn bench_equity_multi_2p_1k(crit: &mut Criterion) {
    let hands = vec![
        Hand(
            c(CardNumber::Ace, CardSuit::Spades),
            c(CardNumber::King, CardSuit::Spades),
        ),
        Hand(
            c(CardNumber::Queen, CardSuit::Hearts),
            c(CardNumber::Queen, CardSuit::Diamonds),
        ),
    ];
    let board = Board {
        flop: None,
        turn: None,
        river: None,
    };

    crit.bench_function("equity_multi_2p_1k", |b| {
        b.iter(|| {
            black_box(calculate_equity_multi(
                black_box(&hands),
                black_box(&board),
                1_000,
            ))
        })
    });
}

/// Benchmark `calculate_equity_multi` — 4 players, 1 000 iterations.
fn bench_equity_multi_4p_1k(crit: &mut Criterion) {
    let hands = vec![
        Hand(
            c(CardNumber::Ace, CardSuit::Spades),
            c(CardNumber::King, CardSuit::Spades),
        ),
        Hand(
            c(CardNumber::Queen, CardSuit::Hearts),
            c(CardNumber::Queen, CardSuit::Diamonds),
        ),
        Hand(
            c(CardNumber::Jack, CardSuit::Clubs),
            c(CardNumber::Ten, CardSuit::Clubs),
        ),
        Hand(
            c(CardNumber::Seven, CardSuit::Hearts),
            c(CardNumber::Six, CardSuit::Hearts),
        ),
    ];
    let board = Board {
        flop: None,
        turn: None,
        river: None,
    };

    crit.bench_function("equity_multi_4p_1k", |b| {
        b.iter(|| {
            black_box(calculate_equity_multi(
                black_box(&hands),
                black_box(&board),
                1_000,
            ))
        })
    });
}

criterion_group!(
    benches,
    bench_hand_best,
    bench_hand_best_high_card,
    bench_full_hand_rank,
    bench_full_hand_compare,
    bench_get_all_cards,
    bench_get_all_numbers,
    bench_equity_1k,
    bench_equity_multi_2p_1k,
    bench_equity_multi_4p_1k,
);
criterion_main!(benches);
