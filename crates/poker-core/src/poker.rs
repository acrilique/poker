// Copyright (C) 2026 Lluc Simó Margalef <lluc.simo@protonmail.com>
// SPDX-License-Identifier: GPL-3.0-only
//
// This file is part of acrilique/poker.
//
// poker is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, version 3 of the License.
//
// poker is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with poker. If not, see <https://www.gnu.org/licenses/>.

//! Poker hand evaluation module.
//!
//! This module provides types and functions for representing playing cards,
//! evaluating poker hands, and calculating hand equity.
//!
//! # Examples
//!
//! ```
//! use poker_core::poker::{Card, CardNumber, CardSuit, Hand, Board};
//!
//! let hand = Hand(
//!     Card(CardNumber::Ace, CardSuit::Spades),
//!     Card(CardNumber::King, CardSuit::Spades),
//! );
//! ```

use rand::RngExt;
use rand::rng;
use std::fmt;

/// Represents a card suit (Diamonds, Spades, Clubs, Hearts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CardSuit {
    Diamonds,
    Spades,
    Clubs,
    Hearts,
}

impl CardSuit {
    /// All suits in standard order
    pub const ALL: [CardSuit; 4] = [
        CardSuit::Diamonds,
        CardSuit::Spades,
        CardSuit::Clubs,
        CardSuit::Hearts,
    ];

    /// Returns the suit as a display symbol
    pub fn symbol(&self) -> &'static str {
        match self {
            CardSuit::Diamonds => "♦",
            CardSuit::Spades => "♠",
            CardSuit::Clubs => "♣",
            CardSuit::Hearts => "♥",
        }
    }
}

/// Represents a card rank (2-14, where 14 = Ace).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CardNumber {
    Two = 2,
    Three = 3,
    Four = 4,
    Five = 5,
    Six = 6,
    Seven = 7,
    Eight = 8,
    Nine = 9,
    Ten = 10,
    Jack = 11,
    Queen = 12,
    King = 13,
    Ace = 14,
}

impl CardNumber {
    /// All ranks in ascending order.
    pub const ALL: [CardNumber; 13] = [
        CardNumber::Two,
        CardNumber::Three,
        CardNumber::Four,
        CardNumber::Five,
        CardNumber::Six,
        CardNumber::Seven,
        CardNumber::Eight,
        CardNumber::Nine,
        CardNumber::Ten,
        CardNumber::Jack,
        CardNumber::Queen,
        CardNumber::King,
        CardNumber::Ace,
    ];

    /// Returns the rank as a display character
    pub fn symbol(&self) -> &'static str {
        match self {
            CardNumber::Two => "2",
            CardNumber::Three => "3",
            CardNumber::Four => "4",
            CardNumber::Five => "5",
            CardNumber::Six => "6",
            CardNumber::Seven => "7",
            CardNumber::Eight => "8",
            CardNumber::Nine => "9",
            CardNumber::Ten => "T",
            CardNumber::Jack => "J",
            CardNumber::Queen => "Q",
            CardNumber::King => "K",
            CardNumber::Ace => "A",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Card(pub CardNumber, pub CardSuit);

impl fmt::Display for Card {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.0.symbol(), self.1.symbol())
    }
}

impl Card {
    pub fn number(&self) -> CardNumber {
        self.0
    }

    pub fn suit(&self) -> CardSuit {
        self.1
    }
}

#[allow(dead_code)]
pub struct Deck(pub [Card; 52]);

/// Represents the community cards on the board.
///
/// The board can have up to 5 cards:
/// - Flop: 3 cards
/// - Turn: 1 additional card
/// - River: 1 final card
pub struct Board {
    pub flop: Option<(Card, Card, Card)>,
    pub turn: Option<Card>,
    pub river: Option<Card>,
}

impl Board {
    /// Collect all community cards into a stack-allocated array.
    /// Returns `(cards, count)` where only `cards[..count]` is valid.
    pub fn cards(&self) -> ([Card; 5], usize) {
        let dummy = Card(CardNumber::Two, CardSuit::Diamonds);
        let mut cards = [dummy; 5];
        let mut len = 0;
        if let Some((c1, c2, c3)) = self.flop {
            cards[0] = c1;
            cards[1] = c2;
            cards[2] = c3;
            len = 3;
        }
        if let Some(c) = self.turn {
            cards[len] = c;
            len += 1;
        }
        if let Some(c) = self.river {
            cards[len] = c;
            len += 1;
        }
        (cards, len)
    }

    /// Fill missing board cards from a deck (mutates deck by popping cards)
    pub fn fill_from_deck(&self, deck: &mut Vec<Card>) -> Board {
        let flop = self
            .flop
            .or_else(|| Some((deck.pop()?, deck.pop()?, deck.pop()?)));
        let turn = self.turn.or_else(|| deck.pop());
        let river = self.river.or_else(|| deck.pop());
        Board { flop, turn, river }
    }
}

/// Represents a player's hole cards (2 private cards).
pub struct Hand(pub Card, pub Card);

/// Represents a complete 5-card poker hand for evaluation.
pub struct FullHand(pub Card, pub Card, pub Card, pub Card, pub Card);

/// Represents the ranking of a poker hand, from lowest to highest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HandRank {
    /// No made hand, only high card.
    HighCard,
    /// Two cards of the same rank.
    Pair,
    /// Two different pairs.
    TwoPair,
    /// Three cards of the same rank.
    ThreeOfAKind,
    /// Five consecutive cards of different suits.
    Straight,
    /// Five cards of the same suit.
    Flush,
    /// Three of a kind plus a pair.
    FullHouse,
    /// Four cards of the same rank.
    FourOfAKind,
    /// Five consecutive cards of the same suit.
    StraightFlush,
    /// A-K-Q-J-T of the same suit.
    RoyalFlush,
}

impl fmt::Display for HandRank {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HandRank::HighCard => write!(f, "High Card"),
            HandRank::Pair => write!(f, "Pair"),
            HandRank::TwoPair => write!(f, "Two Pair"),
            HandRank::ThreeOfAKind => write!(f, "Three of a Kind"),
            HandRank::Straight => write!(f, "Straight"),
            HandRank::Flush => write!(f, "Flush"),
            HandRank::FullHouse => write!(f, "Full House"),
            HandRank::FourOfAKind => write!(f, "Four of a Kind"),
            HandRank::StraightFlush => write!(f, "Straight Flush"),
            HandRank::RoyalFlush => write!(f, "Royal Flush"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Winner {
    Hand1,
    Hand2,
    Tie,
}

impl FullHand {
    /// Determine the rank of this hand.
    ///
    /// Uses popcount + leading/trailing zeros for O(1) straight detection
    /// and (distinct, max_count) to classify the hand without any second loop.
    #[inline]
    pub fn rank(&self) -> HandRank {
        let cards = [self.0, self.1, self.2, self.3, self.4];

        let mut rank_bits: u16 = 0;
        let mut max_count: u8 = 1;
        let mut rank_count = [0u8; 15]; // indexed by rank value (2..=14)
        let first_suit = cards[0].suit() as u8;
        let mut all_same_suit = true;

        for c in &cards {
            let r = c.number() as usize;
            rank_count[r] += 1;
            if rank_count[r] > max_count {
                max_count = rank_count[r];
            }
            rank_bits |= 1 << r;
            if c.suit() as u8 != first_suit {
                all_same_suit = false;
            }
        }

        // O(1) straight detection via popcount + bit-span.
        // A straight has 5 distinct ranks spanning exactly 4 (high - low),
        // or it's a wheel (A-2-3-4-5).
        let distinct = rank_bits.count_ones();
        let is_straight = distinct == 5 && {
            let lo = rank_bits.trailing_zeros();
            let hi = 15 - rank_bits.leading_zeros();
            (hi - lo == 4) || (rank_bits == WHEEL_MASK)
        };

        // (distinct, max_count) uniquely identifies every 5-card pattern:
        //   (5,1) = high card / straight / flush / SF / RF
        //   (4,2) = pair
        //   (3,2) = two pair
        //   (3,3) = three of a kind
        //   (2,3) = full house
        //   (2,4) = four of a kind
        match (all_same_suit, is_straight, distinct, max_count) {
            (true, true, _, _) if rank_bits & ROYAL_MASK == ROYAL_MASK => HandRank::RoyalFlush,
            (true, true, _, _) => HandRank::StraightFlush,
            (_, _, 2, 4) => HandRank::FourOfAKind,
            (_, _, 2, 3) => HandRank::FullHouse,
            (true, _, _, _) => HandRank::Flush,
            (_, true, _, _) => HandRank::Straight,
            (_, _, 3, 3) => HandRank::ThreeOfAKind,
            (_, _, 3, 2) => HandRank::TwoPair,
            (_, _, 4, 2) => HandRank::Pair,
            _ => HandRank::HighCard,
        }
    }

    /// Check if this 5-card hand is a wheel (A-2-3-4-5) using bitmask.
    #[inline]
    fn is_wheel(&self) -> bool {
        let mut rank_bits: u16 = 0;
        for c in [self.0, self.1, self.2, self.3, self.4] {
            rank_bits |= 1 << (c.number() as u16);
        }
        rank_bits & WHEEL_MASK == WHEEL_MASK
    }

    /// Get card numbers grouped by count (for tiebreakers).
    /// Groups are sorted by count desc, then rank desc.
    fn get_ranked_groups(&self) -> [CardNumber; 5] {
        let cards = [self.0, self.1, self.2, self.3, self.4];
        let mut rank_count = [0u8; 15];
        for c in &cards {
            rank_count[c.number() as usize] += 1;
        }

        // Collect only the ranks present in our 5 cards (max 5 unique).
        let mut groups: [(u8, u8); 5] = [(0, 0); 5]; // (count, rank)
        let mut glen = 0;
        let mut seen_bits: u16 = 0;
        for c in &cards {
            let r = c.number() as u16;
            if seen_bits & (1 << r) == 0 {
                seen_bits |= 1 << r;
                groups[glen] = (rank_count[r as usize], r as u8);
                glen += 1;
            }
        }
        // Small sort (2-5 elements): by count desc, then rank desc.
        groups[..glen].sort_unstable_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));

        let mut result = [CardNumber::Two; 5];
        for i in 0..glen {
            result[i] = rank_from_val(groups[i].1 as usize);
        }
        result
    }

    /// Compare two hands and return the winner
    pub fn compare(&self, other: &FullHand) -> Winner {
        let self_rank = self.rank();
        let other_rank = other.rank();

        match self_rank.cmp(&other_rank) {
            std::cmp::Ordering::Greater => Winner::Hand1,
            std::cmp::Ordering::Less => Winner::Hand2,
            std::cmp::Ordering::Equal => {
                // Tiebreaker: compare by grouped ranks
                let self_groups = self.get_ranked_groups();
                let other_groups = other.get_ranked_groups();

                // Special case: wheel straight (A plays low)
                if (self_rank == HandRank::Straight || self_rank == HandRank::StraightFlush)
                    && self.is_wheel() != other.is_wheel()
                {
                    return if self.is_wheel() {
                        Winner::Hand2
                    } else {
                        Winner::Hand1
                    };
                }

                for (s, o) in self_groups.iter().zip(other_groups.iter()) {
                    match s.cmp(o) {
                        std::cmp::Ordering::Greater => return Winner::Hand1,
                        std::cmp::Ordering::Less => return Winner::Hand2,
                        std::cmp::Ordering::Equal => continue,
                    }
                }
                Winner::Tie
            }
        }
    }
}

/// Compare two hands given their boards and return the winner
#[allow(dead_code)]
pub fn determine_winner(hand1: &Hand, hand2: &Hand, board: &Board) -> Option<Winner> {
    let full1 = hand1.best(board)?;
    let full2 = hand2.best(board)?;
    Some(full1.compare(&full2))
}

impl Hand {
    /// Collects all available cards (hand + board) into a stack-allocated array.
    /// Returns `(cards, count)` where only `cards[..count]` is valid (2–7 cards).
    #[inline]
    fn all_cards(&self, board: &Board) -> ([Card; 7], usize) {
        let dummy = Card(CardNumber::Two, CardSuit::Diamonds);
        let mut cards = [dummy; 7];
        cards[0] = self.0;
        cards[1] = self.1;
        let (bc, blen) = board.cards();
        cards[2..2 + blen].copy_from_slice(&bc[..blen]);
        (cards, 2 + blen)
    }

    /// Returns the best possible 5-card hand using single-pass bitmask evaluation.
    ///
    /// Instead of trying each hand type sequentially (10 passes), this method:
    /// 1. Sorts the 7 cards once
    /// 2. Builds rank counts, suit counts, and bitmasks in one pass
    /// 3. Uses the pre-computed data to directly determine the hand type
    /// 4. Selects the optimal 5 cards for that hand type
    pub fn best(&self, board: &Board) -> Option<FullHand> {
        let (cards_arr, cards_len) = self.all_cards(board);
        if cards_len < 5 {
            return None;
        }

        // === One sort for all hand types ===
        let mut sorted_arr = cards_arr;
        let sorted = &mut sorted_arr[..cards_len];
        sorted.sort_by_key(|c| std::cmp::Reverse(c.number()));

        // === Single pass: build rank counts, suit counts, and bitmasks ===
        let mut rank_count = [0u8; 15]; // indexed by rank value 2..=14
        let mut suit_count = [0u8; 4]; // indexed by suit ordinal
        let mut suit_rank_bits = [0u16; 4]; // rank bitmask per suit
        let mut all_rank_bits: u16 = 0;

        for card in sorted.iter() {
            let r = card.number() as usize;
            let s = suit_ordinal(card.suit());
            rank_count[r] += 1;
            suit_count[s] += 1;
            suit_rank_bits[s] |= 1 << r;
            all_rank_bits |= 1 << r;
        }

        // === Classify groups (iterate high-to-low for best-first ordering) ===
        let mut quads_rank: Option<CardNumber> = None;
        let mut trips_rank: [Option<CardNumber>; 2] = [None; 2];
        let mut trips_count = 0usize;
        let mut pair_rank: [Option<CardNumber>; 3] = [None; 3];
        let mut pair_count = 0usize;

        for r in (2..=14usize).rev() {
            match rank_count[r] {
                4 => {
                    if quads_rank.is_none() {
                        quads_rank = Some(rank_from_val(r));
                    }
                }
                3 => {
                    if trips_count < 2 {
                        trips_rank[trips_count] = Some(rank_from_val(r));
                        trips_count += 1;
                    }
                }
                2 => {
                    if pair_count < 3 {
                        pair_rank[pair_count] = Some(rank_from_val(r));
                        pair_count += 1;
                    }
                }
                _ => {}
            }
        }

        // === Check for flush ===
        let flush_suit_idx = suit_count.iter().position(|&c| c >= 5);

        // --- Straight Flush / Royal Flush ---
        if let Some(fsi) = flush_suit_idx
            && let Some(high) = find_straight_high(suit_rank_bits[fsi])
        {
            let flush_suit = CardSuit::ALL[fsi];
            return Some(Self::build_straight(sorted, high, Some(flush_suit)));
        }

        // --- Four of a Kind ---
        if let Some(qr) = quads_rank {
            return Some(Self::build_quads(sorted, qr));
        }

        // --- Full House (trips + pair, or two trips using second as pair) ---
        if trips_count >= 1 {
            let tr = trips_rank[0].unwrap();
            let pair_r = if trips_count >= 2 {
                trips_rank[1]
            } else if pair_count >= 1 {
                pair_rank[0]
            } else {
                None
            };
            if let Some(pr) = pair_r {
                return Some(Self::build_full_house(sorted, tr, pr));
            }
        }

        // --- Flush ---
        if let Some(fsi) = flush_suit_idx {
            let flush_suit = CardSuit::ALL[fsi];
            return Some(Self::build_flush(sorted, flush_suit));
        }

        // --- Straight ---
        if let Some(high) = find_straight_high(all_rank_bits) {
            return Some(Self::build_straight(sorted, high, None));
        }

        // --- Three of a Kind ---
        if trips_count >= 1 {
            let tr = trips_rank[0].unwrap();
            return Some(Self::build_trips(sorted, tr));
        }

        // --- Two Pair ---
        if pair_count >= 2 {
            return Some(Self::build_two_pair(
                sorted,
                pair_rank[0].unwrap(),
                pair_rank[1].unwrap(),
            ));
        }

        // --- One Pair ---
        if pair_count == 1 {
            return Some(Self::build_one_pair(sorted, pair_rank[0].unwrap()));
        }

        // --- High Card ---
        Some(FullHand(
            sorted[0], sorted[1], sorted[2], sorted[3], sorted[4],
        ))
    }

    // === Build helpers — each constructs the optimal FullHand for its type ===

    /// Build a straight (or straight flush if `suit` is Some).
    #[inline]
    fn build_straight(sorted: &[Card], high: CardNumber, suit: Option<CardSuit>) -> FullHand {
        let mut result = [sorted[0]; 5];
        let suit_filter = |c: &&Card| suit.is_none_or(|s| c.suit() == s);
        if high == CardNumber::Five {
            // Wheel: 5-4-3-2-A
            let ranks = [
                CardNumber::Five,
                CardNumber::Four,
                CardNumber::Three,
                CardNumber::Two,
                CardNumber::Ace,
            ];
            for (i, &r) in ranks.iter().enumerate() {
                result[i] = *sorted
                    .iter()
                    .find(|c| c.number() == r && suit_filter(c))
                    .unwrap();
            }
        } else {
            let hv = high as i32;
            for i in 0..5i32 {
                let target = rank_from_val((hv - i) as usize);
                result[i as usize] = *sorted
                    .iter()
                    .find(|c| c.number() == target && suit_filter(c))
                    .unwrap();
            }
        }
        FullHand(result[0], result[1], result[2], result[3], result[4])
    }

    #[inline]
    fn build_quads(sorted: &[Card], quad_rank: CardNumber) -> FullHand {
        let mut result = [sorted[0]; 5];
        let mut idx = 0;
        for c in sorted.iter().filter(|c| c.number() == quad_rank) {
            result[idx] = *c;
            idx += 1;
        }
        if let Some(c) = sorted.iter().find(|c| c.number() != quad_rank) {
            result[idx] = *c;
        }
        FullHand(result[0], result[1], result[2], result[3], result[4])
    }

    #[inline]
    fn build_full_house(sorted: &[Card], trips: CardNumber, pair: CardNumber) -> FullHand {
        let mut result = [sorted[0]; 5];
        let mut idx = 0;
        for c in sorted.iter().filter(|c| c.number() == trips).take(3) {
            result[idx] = *c;
            idx += 1;
        }
        for c in sorted.iter().filter(|c| c.number() == pair).take(2) {
            result[idx] = *c;
            idx += 1;
        }
        FullHand(result[0], result[1], result[2], result[3], result[4])
    }

    #[inline]
    fn build_flush(sorted: &[Card], suit: CardSuit) -> FullHand {
        let mut result = [sorted[0]; 5];
        for (idx, c) in sorted.iter().filter(|c| c.suit() == suit).take(5).enumerate() {
            result[idx] = *c;
        }
        FullHand(result[0], result[1], result[2], result[3], result[4])
    }

    #[inline]
    fn build_trips(sorted: &[Card], trips: CardNumber) -> FullHand {
        let mut result = [sorted[0]; 5];
        let mut idx = 0;
        for c in sorted.iter().filter(|c| c.number() == trips).take(3) {
            result[idx] = *c;
            idx += 1;
        }
        for c in sorted.iter().filter(|c| c.number() != trips).take(2) {
            result[idx] = *c;
            idx += 1;
        }
        FullHand(result[0], result[1], result[2], result[3], result[4])
    }

    #[inline]
    fn build_two_pair(sorted: &[Card], p1: CardNumber, p2: CardNumber) -> FullHand {
        let mut result = [sorted[0]; 5];
        let mut idx = 0;
        for c in sorted.iter().filter(|c| c.number() == p1).take(2) {
            result[idx] = *c;
            idx += 1;
        }
        for c in sorted.iter().filter(|c| c.number() == p2).take(2) {
            result[idx] = *c;
            idx += 1;
        }
        for c in sorted
            .iter()
            .filter(|c| c.number() != p1 && c.number() != p2)
            .take(1)
        {
            result[idx] = *c;
        }
        FullHand(result[0], result[1], result[2], result[3], result[4])
    }

    #[inline]
    fn build_one_pair(sorted: &[Card], pair: CardNumber) -> FullHand {
        let mut result = [sorted[0]; 5];
        let mut idx = 0;
        for c in sorted.iter().filter(|c| c.number() == pair).take(2) {
            result[idx] = *c;
            idx += 1;
        }
        for c in sorted.iter().filter(|c| c.number() != pair).take(3) {
            result[idx] = *c;
            idx += 1;
        }
        FullHand(result[0], result[1], result[2], result[3], result[4])
    }
}

/// Bitmask for the wheel straight (A-5-4-3-2).
const WHEEL_MASK: u16 = (1 << 14) | (1 << 5) | (1 << 4) | (1 << 3) | (1 << 2);
/// Bitmask for a royal flush (A-K-Q-J-T).
const ROYAL_MASK: u16 = (1 << 14) | (1 << 13) | (1 << 12) | (1 << 11) | (1 << 10);

/// Convert a CardSuit to a 0–3 index for array lookups.
#[inline]
fn suit_ordinal(suit: CardSuit) -> usize {
    match suit {
        CardSuit::Diamonds => 0,
        CardSuit::Spades => 1,
        CardSuit::Clubs => 2,
        CardSuit::Hearts => 3,
    }
}

/// Convert a rank integer (2..=14) back to a CardNumber.
#[inline]
fn rank_from_val(val: usize) -> CardNumber {
    CardNumber::ALL[val - 2]
}

/// Find the highest straight in a rank bitmask (bit *r* set ⇔ rank *r* present).
/// Returns the high card of the straight, or `CardNumber::Five` for a wheel.
#[inline]
fn find_straight_high(rank_bits: u16) -> Option<CardNumber> {
    // Check 5-wide windows from Ace-high (14) down to Six-high (6).
    for high in (6u32..=14).rev() {
        let mask: u16 = 0x1F << (high - 4);
        if rank_bits & mask == mask {
            return Some(rank_from_val(high as usize));
        }
    }
    // Wheel: A-5-4-3-2
    if rank_bits & WHEEL_MASK == WHEEL_MASK {
        return Some(CardNumber::Five);
    }
    None
}

/// Helper function to get all card numbers.
///
/// Returns [`CardNumber::ALL`] by value (13-byte `Copy` array, no heap allocation).
#[inline]
pub fn get_all_numbers() -> [CardNumber; 13] {
    CardNumber::ALL
}

/// All 52 cards, ordered by suit then rank.
pub const ALL_CARDS: [Card; 52] = {
    let mut cards = [Card(CardNumber::Two, CardSuit::Diamonds); 52];
    let mut i = 0;
    let suits = CardSuit::ALL;
    let numbers = CardNumber::ALL;
    let mut s = 0;
    while s < suits.len() {
        let mut n = 0;
        while n < numbers.len() {
            cards[i] = Card(numbers[n], suits[s]);
            i += 1;
            n += 1;
        }
        s += 1;
    }
    cards
};

/// Returns [`ALL_CARDS`] by value.
#[inline]
pub fn get_all_cards() -> [Card; 52] {
    ALL_CARDS
}

#[allow(dead_code)]
pub fn calculate_equity(hero: &Hand, board: &Board, iterations: usize) -> (f64, f64, f64) {
    let mut wins = 0;
    let mut ties = 0;
    let mut losses = 0;
    let mut rng = rng();

    // Identify known cards (stack-allocated)
    let mut known = [Card(CardNumber::Two, CardSuit::Diamonds); 7];
    known[0] = hero.0;
    known[1] = hero.1;
    let (bc, blen) = board.cards();
    known[2..2 + blen].copy_from_slice(&bc[..blen]);
    let known_len = 2 + blen;

    // Pre-build the deck template once (stack-allocated, zero heap alloc)
    let all_cards = get_all_cards();
    let mut deck_template = [Card(CardNumber::Two, CardSuit::Diamonds); 52];
    let mut deck_size = 0;
    for c in &all_cards {
        if !known[..known_len].contains(c) {
            deck_template[deck_size] = *c;
            deck_size += 1;
        }
    }

    // How many random cards we need per iteration:
    // 2 (villain) + missing board cards
    let board_missing = 5 - blen;
    let cards_needed = 2 + board_missing;

    for _ in 0..iterations {
        // Copy template (stack → stack, no heap)
        let mut deck = deck_template;

        // Partial Fisher-Yates: only randomise the positions we actually deal
        for i in 0..cards_needed {
            let j = rng.random_range(i..deck_size);
            deck.swap(i, j);
        }

        // Deal villain from first 2 positions
        let villain = Hand(deck[0], deck[1]);

        // Build simulated board from positions 2..
        let mut di = 2;
        let sim_flop = match board.flop {
            Some(f) => f,
            None => {
                let f = (deck[di], deck[di + 1], deck[di + 2]);
                di += 3;
                f
            }
        };
        let sim_turn = match board.turn {
            Some(t) => t,
            None => {
                let t = deck[di];
                di += 1;
                t
            }
        };
        let sim_river = match board.river {
            Some(r) => r,
            None => deck[di],
        };
        let sim_board = Board {
            flop: Some(sim_flop),
            turn: Some(sim_turn),
            river: Some(sim_river),
        };

        match determine_winner(hero, &villain, &sim_board) {
            Some(Winner::Hand1) => wins += 1,
            Some(Winner::Hand2) => losses += 1,
            Some(Winner::Tie) => ties += 1,
            None => {}
        }
    }

    (
        wins as f64 / iterations as f64,
        ties as f64 / iterations as f64,
        losses as f64 / iterations as f64,
    )
}

/// Calculate equity for multiple hands in an all-in situation
/// Returns a vector of (win_equity, tie_equity) for each hand in the same order
pub fn calculate_equity_multi(hands: &[Hand], board: &Board, iterations: usize) -> Vec<f64> {
    if hands.is_empty() {
        return vec![];
    }
    if hands.len() == 1 {
        return vec![100.0];
    }

    let mut wins: Vec<usize> = vec![0; hands.len()];
    let mut ties: Vec<usize> = vec![0; hands.len()];
    let mut rng = rng();

    // Identify known cards (stack-allocated — max 23 cards: 9 players × 2 + 5 board)
    let mut known = [Card(CardNumber::Two, CardSuit::Diamonds); 23];
    let mut known_len = 0;
    for h in hands {
        known[known_len] = h.0;
        known[known_len + 1] = h.1;
        known_len += 2;
    }
    let (bc, blen) = board.cards();
    known[known_len..known_len + blen].copy_from_slice(&bc[..blen]);
    known_len += blen;

    // Pre-build the deck template once
    let all_cards = get_all_cards();
    let mut deck_template = [Card(CardNumber::Two, CardSuit::Diamonds); 52];
    let mut deck_size = 0;
    for c in &all_cards {
        if !known[..known_len].contains(c) {
            deck_template[deck_size] = *c;
            deck_size += 1;
        }
    }

    // Only need to deal missing board cards (all player hands are known)
    let board_missing = 5 - blen;

    // Pre-allocate per-iteration scratch (avoids Vec allocation in the hot loop)
    let num_hands = hands.len();
    let mut best_hands = Vec::with_capacity(num_hands);
    let mut winner_indices = Vec::with_capacity(num_hands);

    for _ in 0..iterations {
        let mut deck = deck_template;

        // Partial Fisher-Yates for board cards only
        for i in 0..board_missing {
            let j = rng.random_range(i..deck_size);
            deck.swap(i, j);
        }

        // Build simulated board
        let mut di = 0;
        let sim_flop = match board.flop {
            Some(f) => f,
            None => {
                let f = (deck[di], deck[di + 1], deck[di + 2]);
                di += 3;
                f
            }
        };
        let sim_turn = match board.turn {
            Some(t) => t,
            None => {
                let t = deck[di];
                di += 1;
                t
            }
        };
        let sim_river = match board.river {
            Some(r) => r,
            None => deck[di],
        };
        let sim_board = Board {
            flop: Some(sim_flop),
            turn: Some(sim_turn),
            river: Some(sim_river),
        };

        // Evaluate all hands and find winner(s)
        best_hands.clear();
        for (i, hand) in hands.iter().enumerate() {
            if let Some(full_hand) = hand.best(&sim_board) {
                best_hands.push((i, full_hand));
            }
        }

        if best_hands.is_empty() {
            continue;
        }

        // Find the best hand(s)
        winner_indices.clear();
        winner_indices.push(best_hands[0].0);
        let mut best = &best_hands[0].1;

        for (i, full_hand) in best_hands.iter().skip(1) {
            match full_hand.compare(best) {
                Winner::Hand1 => {
                    winner_indices.clear();
                    winner_indices.push(*i);
                    best = full_hand;
                }
                Winner::Tie => {
                    winner_indices.push(*i);
                }
                Winner::Hand2 => {}
            }
        }

        if winner_indices.len() == 1 {
            wins[winner_indices[0]] += 1;
        } else {
            for &idx in &winner_indices {
                ties[idx] += 1;
            }
        }
    }

    // Calculate equity as win% + (tie% / number_of_tiers)
    hands
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let win_pct = (wins[i] as f64 / iterations as f64) * 100.0;
            let tie_pct = (ties[i] as f64 / iterations as f64) * 100.0;
            // For ties, equity is split proportionally among tied players
            // Approximate by dividing tie equity by average number of players in ties
            win_pct + (tie_pct / 2.0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to create cards easily
    fn c(rank: CardNumber, suit: CardSuit) -> Card {
        Card(rank, suit)
    }

    fn make_board(flop: Option<[Card; 3]>, turn: Option<Card>, river: Option<Card>) -> Board {
        Board {
            flop: flop.map(|[a, b, c]| (a, b, c)),
            turn,
            river,
        }
    }

    #[test]
    fn test_card_display() {
        let card = c(CardNumber::Ace, CardSuit::Spades);
        assert_eq!(format!("{}", card), "A♠");

        let card = c(CardNumber::Ten, CardSuit::Hearts);
        assert_eq!(format!("{}", card), "T♥");

        let card = c(CardNumber::Two, CardSuit::Diamonds);
        assert_eq!(format!("{}", card), "2♦");
    }

    #[test]
    fn test_royal_flush() {
        let hand = Hand(
            c(CardNumber::Ace, CardSuit::Spades),
            c(CardNumber::King, CardSuit::Spades),
        );
        let board = make_board(
            Some([
                c(CardNumber::Queen, CardSuit::Spades),
                c(CardNumber::Jack, CardSuit::Spades),
                c(CardNumber::Ten, CardSuit::Spades),
            ]),
            Some(c(CardNumber::Two, CardSuit::Hearts)),
            Some(c(CardNumber::Three, CardSuit::Clubs)),
        );

        let best = hand.best(&board).unwrap();
        assert_eq!(best.rank(), HandRank::RoyalFlush);
    }

    #[test]
    fn test_straight_flush() {
        let hand = Hand(
            c(CardNumber::Nine, CardSuit::Hearts),
            c(CardNumber::Eight, CardSuit::Hearts),
        );
        let board = make_board(
            Some([
                c(CardNumber::Seven, CardSuit::Hearts),
                c(CardNumber::Six, CardSuit::Hearts),
                c(CardNumber::Five, CardSuit::Hearts),
            ]),
            Some(c(CardNumber::Two, CardSuit::Clubs)),
            Some(c(CardNumber::Three, CardSuit::Diamonds)),
        );

        let best = hand.best(&board).unwrap();
        assert_eq!(best.rank(), HandRank::StraightFlush);
    }

    #[test]
    fn test_four_of_a_kind() {
        let hand = Hand(
            c(CardNumber::King, CardSuit::Spades),
            c(CardNumber::King, CardSuit::Hearts),
        );
        let board = make_board(
            Some([
                c(CardNumber::King, CardSuit::Diamonds),
                c(CardNumber::King, CardSuit::Clubs),
                c(CardNumber::Ace, CardSuit::Spades),
            ]),
            Some(c(CardNumber::Two, CardSuit::Hearts)),
            Some(c(CardNumber::Three, CardSuit::Clubs)),
        );

        let best = hand.best(&board).unwrap();
        assert_eq!(best.rank(), HandRank::FourOfAKind);
    }

    #[test]
    fn test_full_house() {
        let hand = Hand(
            c(CardNumber::Queen, CardSuit::Spades),
            c(CardNumber::Queen, CardSuit::Hearts),
        );
        let board = make_board(
            Some([
                c(CardNumber::Queen, CardSuit::Diamonds),
                c(CardNumber::Jack, CardSuit::Clubs),
                c(CardNumber::Jack, CardSuit::Spades),
            ]),
            Some(c(CardNumber::Two, CardSuit::Hearts)),
            Some(c(CardNumber::Three, CardSuit::Clubs)),
        );

        let best = hand.best(&board).unwrap();
        assert_eq!(best.rank(), HandRank::FullHouse);
    }

    #[test]
    fn test_flush() {
        let hand = Hand(
            c(CardNumber::Ace, CardSuit::Clubs),
            c(CardNumber::Ten, CardSuit::Clubs),
        );
        let board = make_board(
            Some([
                c(CardNumber::Seven, CardSuit::Clubs),
                c(CardNumber::Four, CardSuit::Clubs),
                c(CardNumber::Two, CardSuit::Clubs),
            ]),
            Some(c(CardNumber::King, CardSuit::Hearts)),
            Some(c(CardNumber::Three, CardSuit::Diamonds)),
        );

        let best = hand.best(&board).unwrap();
        assert_eq!(best.rank(), HandRank::Flush);
    }

    #[test]
    fn test_straight() {
        let hand = Hand(
            c(CardNumber::Nine, CardSuit::Spades),
            c(CardNumber::Eight, CardSuit::Hearts),
        );
        let board = make_board(
            Some([
                c(CardNumber::Seven, CardSuit::Clubs),
                c(CardNumber::Six, CardSuit::Diamonds),
                c(CardNumber::Five, CardSuit::Spades),
            ]),
            Some(c(CardNumber::Two, CardSuit::Hearts)),
            Some(c(CardNumber::King, CardSuit::Clubs)),
        );

        let best = hand.best(&board).unwrap();
        assert_eq!(best.rank(), HandRank::Straight);
    }

    #[test]
    fn test_wheel_straight() {
        // A-2-3-4-5 (wheel)
        let hand = Hand(
            c(CardNumber::Ace, CardSuit::Spades),
            c(CardNumber::Two, CardSuit::Hearts),
        );
        let board = make_board(
            Some([
                c(CardNumber::Three, CardSuit::Clubs),
                c(CardNumber::Four, CardSuit::Diamonds),
                c(CardNumber::Five, CardSuit::Spades),
            ]),
            Some(c(CardNumber::King, CardSuit::Hearts)),
            Some(c(CardNumber::Queen, CardSuit::Clubs)),
        );

        let best = hand.best(&board).unwrap();
        assert_eq!(best.rank(), HandRank::Straight);
    }

    #[test]
    fn test_three_of_a_kind() {
        let hand = Hand(
            c(CardNumber::Jack, CardSuit::Spades),
            c(CardNumber::Jack, CardSuit::Hearts),
        );
        let board = make_board(
            Some([
                c(CardNumber::Jack, CardSuit::Diamonds),
                c(CardNumber::Ace, CardSuit::Clubs),
                c(CardNumber::King, CardSuit::Spades),
            ]),
            Some(c(CardNumber::Two, CardSuit::Hearts)),
            Some(c(CardNumber::Three, CardSuit::Clubs)),
        );

        let best = hand.best(&board).unwrap();
        assert_eq!(best.rank(), HandRank::ThreeOfAKind);
    }

    #[test]
    fn test_two_pair() {
        let hand = Hand(
            c(CardNumber::Ace, CardSuit::Spades),
            c(CardNumber::Ace, CardSuit::Hearts),
        );
        let board = make_board(
            Some([
                c(CardNumber::King, CardSuit::Diamonds),
                c(CardNumber::King, CardSuit::Clubs),
                c(CardNumber::Two, CardSuit::Spades),
            ]),
            Some(c(CardNumber::Three, CardSuit::Hearts)),
            Some(c(CardNumber::Four, CardSuit::Clubs)),
        );

        let best = hand.best(&board).unwrap();
        assert_eq!(best.rank(), HandRank::TwoPair);
    }

    #[test]
    fn test_pair() {
        let hand = Hand(
            c(CardNumber::Queen, CardSuit::Spades),
            c(CardNumber::Queen, CardSuit::Hearts),
        );
        let board = make_board(
            Some([
                c(CardNumber::Ace, CardSuit::Diamonds),
                c(CardNumber::King, CardSuit::Clubs),
                c(CardNumber::Jack, CardSuit::Spades),
            ]),
            Some(c(CardNumber::Two, CardSuit::Hearts)),
            Some(c(CardNumber::Three, CardSuit::Clubs)),
        );

        let best = hand.best(&board).unwrap();
        assert_eq!(best.rank(), HandRank::Pair);
    }

    #[test]
    fn test_high_card() {
        let hand = Hand(
            c(CardNumber::Ace, CardSuit::Spades),
            c(CardNumber::King, CardSuit::Hearts),
        );
        let board = make_board(
            Some([
                c(CardNumber::Nine, CardSuit::Diamonds),
                c(CardNumber::Seven, CardSuit::Clubs),
                c(CardNumber::Four, CardSuit::Spades),
            ]),
            Some(c(CardNumber::Two, CardSuit::Hearts)),
            Some(c(CardNumber::Three, CardSuit::Clubs)),
        );

        let best = hand.best(&board).unwrap();
        assert_eq!(best.rank(), HandRank::HighCard);
    }

    #[test]
    fn test_hand_comparison_different_ranks() {
        // Full house vs flush
        let hand1 = Hand(
            c(CardNumber::King, CardSuit::Spades),
            c(CardNumber::King, CardSuit::Hearts),
        );
        let hand2 = Hand(
            c(CardNumber::Ace, CardSuit::Clubs),
            c(CardNumber::Ten, CardSuit::Clubs),
        );
        let board = make_board(
            Some([
                c(CardNumber::King, CardSuit::Diamonds),
                c(CardNumber::Queen, CardSuit::Clubs),
                c(CardNumber::Jack, CardSuit::Clubs),
            ]),
            Some(c(CardNumber::Nine, CardSuit::Clubs)),
            Some(c(CardNumber::Queen, CardSuit::Hearts)),
        );

        let full1 = hand1.best(&board).unwrap();
        let full2 = hand2.best(&board).unwrap();

        assert_eq!(full1.rank(), HandRank::FullHouse);
        assert_eq!(full2.rank(), HandRank::Flush);
        assert_eq!(full1.compare(&full2), Winner::Hand1);
    }

    #[test]
    fn test_hand_comparison_same_rank_different_kicker() {
        // Both have pair of aces, but different kickers
        let hand1 = Hand(
            c(CardNumber::Ace, CardSuit::Spades),
            c(CardNumber::King, CardSuit::Hearts),
        );
        let hand2 = Hand(
            c(CardNumber::Ace, CardSuit::Diamonds),
            c(CardNumber::Queen, CardSuit::Clubs),
        );
        let board = make_board(
            Some([
                c(CardNumber::Ace, CardSuit::Hearts),
                c(CardNumber::Nine, CardSuit::Clubs),
                c(CardNumber::Seven, CardSuit::Spades),
            ]),
            Some(c(CardNumber::Six, CardSuit::Hearts)),
            Some(c(CardNumber::Two, CardSuit::Diamonds)),
        );

        let full1 = hand1.best(&board).unwrap();
        let full2 = hand2.best(&board).unwrap();

        assert_eq!(full1.rank(), HandRank::Pair);
        assert_eq!(full2.rank(), HandRank::Pair);
        assert_eq!(full1.compare(&full2), Winner::Hand1); // King kicker beats Queen
    }

    #[test]
    fn test_hand_comparison_tie() {
        // Both have same straight on the board
        let hand1 = Hand(
            c(CardNumber::Two, CardSuit::Spades),
            c(CardNumber::Three, CardSuit::Hearts),
        );
        let hand2 = Hand(
            c(CardNumber::Two, CardSuit::Diamonds),
            c(CardNumber::Three, CardSuit::Clubs),
        );
        let board = make_board(
            Some([
                c(CardNumber::Ten, CardSuit::Hearts),
                c(CardNumber::Jack, CardSuit::Clubs),
                c(CardNumber::Queen, CardSuit::Spades),
            ]),
            Some(c(CardNumber::King, CardSuit::Hearts)),
            Some(c(CardNumber::Ace, CardSuit::Diamonds)),
        );

        let full1 = hand1.best(&board).unwrap();
        let full2 = hand2.best(&board).unwrap();

        assert_eq!(full1.rank(), HandRank::Straight);
        assert_eq!(full2.rank(), HandRank::Straight);
        assert_eq!(full1.compare(&full2), Winner::Tie);
    }

    #[test]
    fn test_get_all_cards() {
        let cards = get_all_cards();
        assert_eq!(cards.len(), 52);

        // Check we have 4 of each rank
        for number in get_all_numbers() {
            let count = cards.iter().filter(|c| c.number() == number).count();
            assert_eq!(count, 4, "Should have 4 cards of {:?}", number);
        }

        // Check we have 13 of each suit
        for suit in CardSuit::ALL {
            let count = cards.iter().filter(|c| c.suit() == suit).count();
            assert_eq!(count, 13, "Should have 13 cards of {:?}", suit);
        }
    }

    #[test]
    fn test_hand_rank_ordering() {
        assert!(HandRank::RoyalFlush > HandRank::StraightFlush);
        assert!(HandRank::StraightFlush > HandRank::FourOfAKind);
        assert!(HandRank::FourOfAKind > HandRank::FullHouse);
        assert!(HandRank::FullHouse > HandRank::Flush);
        assert!(HandRank::Flush > HandRank::Straight);
        assert!(HandRank::Straight > HandRank::ThreeOfAKind);
        assert!(HandRank::ThreeOfAKind > HandRank::TwoPair);
        assert!(HandRank::TwoPair > HandRank::Pair);
        assert!(HandRank::Pair > HandRank::HighCard);
    }
}
