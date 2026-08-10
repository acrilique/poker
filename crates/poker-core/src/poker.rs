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
    pub const ALL: [Self; 4] = [Self::Diamonds, Self::Spades, Self::Clubs, Self::Hearts];

    /// Numeric index of the suit (0–3): Diamonds=0, Spades=1, Clubs=2,
    /// Hearts=3.
    #[must_use]
    pub const fn value(self) -> u8 {
        match self {
            Self::Diamonds => 0,
            Self::Spades => 1,
            Self::Clubs => 2,
            Self::Hearts => 3,
        }
    }

    /// Whether the suit renders red (Diamonds or Hearts) in a UI.
    #[must_use]
    pub const fn is_red(self) -> bool {
        matches!(self, Self::Diamonds | Self::Hearts)
    }

    /// Returns the suit as a display symbol
    #[must_use]
    pub const fn symbol(&self) -> &'static str {
        match self {
            Self::Diamonds => "♦",
            Self::Spades => "♠",
            Self::Clubs => "♣",
            Self::Hearts => "♥",
        }
    }
}

/// Represents a card rank (2-14, where 14 = Ace).
///
/// The explicit discriminants (2..=14) are the enum's numeric identity;
/// [`Self::value`] is their single source of truth, with [`Self::index`] and
/// [`Self::bit`] widening from it.
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
    pub const ALL: [Self; 13] = [
        Self::Two,
        Self::Three,
        Self::Four,
        Self::Five,
        Self::Six,
        Self::Seven,
        Self::Eight,
        Self::Nine,
        Self::Ten,
        Self::Jack,
        Self::Queen,
        Self::King,
        Self::Ace,
    ];

    /// Numeric value of the rank (2–14, where 14 = Ace).
    #[must_use]
    pub const fn value(self) -> u8 {
        match self {
            Self::Two => 2,
            Self::Three => 3,
            Self::Four => 4,
            Self::Five => 5,
            Self::Six => 6,
            Self::Seven => 7,
            Self::Eight => 8,
            Self::Nine => 9,
            Self::Ten => 10,
            Self::Jack => 11,
            Self::Queen => 12,
            Self::King => 13,
            Self::Ace => 14,
        }
    }

    /// `usize` index matching the rank value (2–14), used to index the
    /// `[u8; 15]` rank-count tables.
    #[must_use]
    pub fn index(self) -> usize {
        self.value().into()
    }

    /// `u16` bitmask bit position for the rank (same as [`Self::index`] but in
    /// `u16` so it can feed `1 << bit` without a cast).
    #[must_use]
    pub fn bit(self) -> u16 {
        self.value().into()
    }

    /// Returns the rank as a display character.
    #[must_use]
    pub const fn symbol(&self) -> &'static str {
        match self {
            Self::Two => "2",
            Self::Three => "3",
            Self::Four => "4",
            Self::Five => "5",
            Self::Six => "6",
            Self::Seven => "7",
            Self::Eight => "8",
            Self::Nine => "9",
            Self::Ten => "10",
            Self::Jack => "J",
            Self::Queen => "Q",
            Self::King => "K",
            Self::Ace => "A",
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
    #[must_use]
    pub const fn number(&self) -> CardNumber {
        self.0
    }

    #[must_use]
    pub const fn suit(&self) -> CardSuit {
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
    #[must_use]
    pub fn cards(&self) -> ([Card; 5], usize) {
        let dummy = Card(CardNumber::Two, CardSuit::Diamonds);
        let mut cards = [dummy; 5];
        // Flop fills the first three slots; without it the board starts empty.
        let mut len = if let Some((c1, c2, c3)) = self.flop {
            if let Some(slot) = cards.get_mut(0) {
                *slot = c1;
            }
            if let Some(slot) = cards.get_mut(1) {
                *slot = c2;
            }
            if let Some(slot) = cards.get_mut(2) {
                *slot = c3;
            }
            3
        } else {
            0
        };
        if let Some(c) = self.turn
            && let Some(slot) = cards.get_mut(len)
        {
            *slot = c;
            len = len.saturating_add(1);
        }
        if let Some(c) = self.river
            && let Some(slot) = cards.get_mut(len)
        {
            *slot = c;
            len = len.saturating_add(1);
        }
        (cards, len)
    }

    /// Fill missing board cards from a deck (mutates deck by popping cards).
    #[must_use]
    pub fn fill_from_deck(&self, deck: &mut Vec<Card>) -> Self {
        let flop = self
            .flop
            .or_else(|| Some((deck.pop()?, deck.pop()?, deck.pop()?)));
        let turn = self.turn.or_else(|| deck.pop());
        let river = self.river.or_else(|| deck.pop());
        Self { flop, turn, river }
    }
}

/// Represents a player's hole cards (2 private cards).
pub struct Hand(pub Card, pub Card);

/// Represents a complete 5-card poker hand for evaluation.
#[derive(Debug, Clone, Copy)]
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
            Self::HighCard => write!(f, "High Card"),
            Self::Pair => write!(f, "Pair"),
            Self::TwoPair => write!(f, "Two Pair"),
            Self::ThreeOfAKind => write!(f, "Three of a Kind"),
            Self::Straight => write!(f, "Straight"),
            Self::Flush => write!(f, "Flush"),
            Self::FullHouse => write!(f, "Full House"),
            Self::FourOfAKind => write!(f, "Four of a Kind"),
            Self::StraightFlush => write!(f, "Straight Flush"),
            Self::RoyalFlush => write!(f, "Royal Flush"),
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
    /// Uses popcount + leading/trailing zeros for `O(1)` straight detection
    /// and `(distinct, max_count)` to classify the hand without any second loop.
    #[inline]
    #[must_use]
    pub fn rank(&self) -> HandRank {
        let cards = [self.0, self.1, self.2, self.3, self.4];

        let mut rank_bits: u16 = 0;
        let mut max_count: u8 = 1;
        let mut rank_count = [0u8; 15]; // indexed by rank value (2..=14)
        let first_suit = cards[0].suit().value();
        let mut all_same_suit = true;

        for c in &cards {
            let r = c.number().index();
            if let Some(slot) = rank_count.get_mut(r) {
                *slot = slot.saturating_add(1);
                if *slot > max_count {
                    max_count = *slot;
                }
            }
            rank_bits |= 1u16 << c.number().bit();
            if c.suit().value() != first_suit {
                all_same_suit = false;
            }
        }

        // O(1) straight detection via popcount + bit-span.
        // A straight has 5 distinct ranks spanning exactly 4 (high - low),
        // or it's a wheel (A-2-3-4-5).
        let distinct = u8::try_from(rank_bits.count_ones()).unwrap_or(u8::MAX);
        let is_straight = distinct == 5 && {
            let lo = rank_bits.trailing_zeros();
            // Highest set bit position = (BITS - 1) - leading_zeros. For u16
            // with the top bit unset this matches the original `15 - leading_zeros`.
            let hi = (u16::BITS - 1).saturating_sub(rank_bits.leading_zeros());
            hi.saturating_sub(lo) == 4 || (rank_bits == WHEEL_MASK)
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
            rank_bits |= 1u16 << c.number().bit();
        }
        rank_bits & WHEEL_MASK == WHEEL_MASK
    }

    /// Get card numbers grouped by count (for tiebreakers).
    /// Groups are sorted by count desc, then rank desc.
    fn get_ranked_groups(&self) -> [CardNumber; 5] {
        let cards = [self.0, self.1, self.2, self.3, self.4];
        let mut rank_count = [0u8; 15];
        for c in &cards {
            if let Some(slot) = rank_count.get_mut(c.number().index()) {
                *slot = slot.saturating_add(1);
            }
        }

        // Collect only the ranks present in our 5 cards (max 5 unique).
        let mut groups: [(u8, u8); 5] = [(0, 0); 5]; // (count, rank)
        let mut glen = 0;
        let mut seen_bits: u16 = 0;
        for c in &cards {
            let r = c.number().bit();
            if seen_bits & (1u16 << r) == 0 {
                seen_bits |= 1u16 << r;
                let count = rank_count.get(c.number().index()).copied().unwrap_or(0);
                // r is 2..=14, which fits in u8 without truncation.
                let rank_val = u8::try_from(r).unwrap_or(0);
                if let Some(slot) = groups.get_mut(glen) {
                    *slot = (count, rank_val);
                    glen = glen.saturating_add(1);
                }
            }
        }
        // Small sort (2-5 elements): by count desc, then rank desc.
        if let Some(slice) = groups.get_mut(..glen) {
            slice.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        }

        let mut result = [CardNumber::Two; 5];
        for (i, group) in groups.iter().enumerate() {
            if i >= glen {
                break;
            }
            // group.1 is a rank value 2..=14; map back to a CardNumber.
            if let Some(slot) = result.get_mut(i) {
                *slot = rank_from_val(usize::from(group.1));
            }
        }
        result
    }

    /// Compare two hands and return the winner.
    #[must_use]
    pub fn compare(&self, other: &Self) -> Winner {
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
                        std::cmp::Ordering::Equal => {}
                    }
                }
                Winner::Tie
            }
        }
    }
}

/// Compare two hands given their boards and return the winner.
#[allow(dead_code)]
#[must_use]
pub fn determine_winner(hand1: &Hand, hand2: &Hand, board: &Board) -> Option<Winner> {
    let full1 = hand1.best(board)?;
    let full2 = hand2.best(board)?;
    Some(full1.compare(&full2))
}

/// Classified rank groups extracted from a rank-count table, used by
/// [`Hand::best`] to pick the optimal 5-card hand.
struct RankGroups {
    quads_rank: Option<CardNumber>,
    trips_rank: [Option<CardNumber>; 2],
    trips_count: usize,
    pair_rank: [Option<CardNumber>; 3],
    pair_count: usize,
}

/// Scan a `[u8; 15]` rank-count table (indexed by rank value 2..=14)
/// high-to-low and collect the best quad, up to two trips, and up to three pairs.
fn classify_groups(rank_count: &[u8; 15]) -> RankGroups {
    let mut quads_rank: Option<CardNumber> = None;
    let mut trips_rank: [Option<CardNumber>; 2] = [None; 2];
    let mut trips_count = 0usize;
    let mut pair_rank: [Option<CardNumber>; 3] = [None; 3];
    let mut pair_count = 0usize;

    for r in (2..=14usize).rev() {
        match rank_count.get(r).copied().unwrap_or(0) {
            4 => {
                if quads_rank.is_none() {
                    quads_rank = Some(rank_from_val(r));
                }
            }
            3 => {
                if trips_count < 2 {
                    if let Some(slot) = trips_rank.get_mut(trips_count) {
                        *slot = Some(rank_from_val(r));
                    }
                    trips_count = trips_count.saturating_add(1);
                }
            }
            2 => {
                if pair_count < 3 {
                    if let Some(slot) = pair_rank.get_mut(pair_count) {
                        *slot = Some(rank_from_val(r));
                    }
                    pair_count = pair_count.saturating_add(1);
                }
            }
            _ => {}
        }
    }

    RankGroups {
        quads_rank,
        trips_rank,
        trips_count,
        pair_rank,
        pair_count,
    }
}

impl Hand {
    /// Collects all available cards (hand + board) into a stack-allocated array.
    /// Returns `(cards, count)` where only `cards[..count]` is valid (2–7 cards).
    #[inline]
    fn all_cards(&self, board: &Board) -> ([Card; 7], usize) {
        let dummy = Card(CardNumber::Two, CardSuit::Diamonds);
        let mut cards = [dummy; 7];
        if let Some(slot) = cards.get_mut(0) {
            *slot = self.0;
        }
        if let Some(slot) = cards.get_mut(1) {
            *slot = self.1;
        }
        let (bc, blen) = board.cards();
        // blen is 0..=5 and the hand occupies slots 0,1; remaining slots 2..7.
        let start: usize = 2;
        let end = start.saturating_add(blen).min(cards.len());
        if end > start
            && let Some(dst) = cards.get_mut(start..end)
            && let Some(src) = bc.get(..end.saturating_sub(start))
        {
            dst.copy_from_slice(src);
        }
        (cards, start.saturating_add(blen))
    }

    /// Returns the best possible 5-card hand using single-pass bitmask evaluation.
    ///
    /// Instead of trying each hand type sequentially (10 passes), this method:
    /// 1. Sorts the 7 cards once
    /// 2. Builds rank counts, suit counts, and bitmasks in one pass
    /// 3. Uses the pre-computed data to directly determine the hand type
    /// 4. Selects the optimal 5 cards for that hand type
    #[must_use]
    pub fn best(&self, board: &Board) -> Option<FullHand> {
        let (cards_arr, cards_len) = self.all_cards(board);
        if cards_len < 5 {
            return None;
        }

        // === One sort for all hand types ===
        let mut sorted_arr = cards_arr;
        // cards_len ≤ sorted_arr.len() by construction (all_cards caps at 7).
        let sorted = sorted_arr.get_mut(..cards_len)?;
        sorted.sort_by_key(|c| std::cmp::Reverse(c.number()));

        // === Single pass: build rank counts, suit counts, and bitmasks ===
        let mut rank_count = [0u8; 15]; // indexed by rank value 2..=14
        let mut suit_count = [0u8; 4]; // indexed by suit ordinal
        let mut suit_rank_bits = [0u16; 4]; // rank bitmask per suit
        let mut all_rank_bits: u16 = 0;

        for card in sorted.iter() {
            let r = card.number().index();
            let s = suit_ordinal(card.suit());
            if let Some(slot) = rank_count.get_mut(r) {
                *slot = slot.saturating_add(1);
            }
            if let Some(slot) = suit_count.get_mut(s) {
                *slot = slot.saturating_add(1);
            }
            if let Some(slot) = suit_rank_bits.get_mut(s) {
                *slot |= 1u16 << card.number().bit();
            }
            all_rank_bits |= 1u16 << card.number().bit();
        }

        // === Classify groups (iterate high-to-low for best-first ordering) ===
        let groups = classify_groups(&rank_count);
        let RankGroups {
            quads_rank,
            trips_rank,
            trips_count,
            pair_rank,
            pair_count,
        } = groups;

        // === Check for flush ===
        let flush_suit_idx = suit_count.iter().position(|&c| c >= 5);

        // --- Straight Flush / Royal Flush ---
        if let Some(fsi) = flush_suit_idx
            && let Some(&bits) = suit_rank_bits.get(fsi)
            && let Some(high) = find_straight_high(bits)
        {
            let flush_suit = CardSuit::ALL
                .get(fsi)
                .copied()
                .unwrap_or(CardSuit::Diamonds);
            return Some(Self::build_straight(sorted, high, Some(flush_suit)));
        }

        // --- Four of a Kind ---
        if let Some(qr) = quads_rank {
            return Some(Self::build_quads(sorted, qr));
        }

        // --- Full House (trips + pair, or two trips using second as pair) ---
        if trips_count >= 1 {
            let tr = trips_rank.first().copied().flatten();
            let pair_r = if trips_count >= 2 {
                trips_rank.get(1).copied().flatten()
            } else if pair_count >= 1 {
                pair_rank.first().copied().flatten()
            } else {
                None
            };
            if let (Some(tr), Some(pr)) = (tr, pair_r) {
                return Some(Self::build_full_house(sorted, tr, pr));
            }
        }

        // --- Flush ---
        if let Some(fsi) = flush_suit_idx {
            let flush_suit = CardSuit::ALL
                .get(fsi)
                .copied()
                .unwrap_or(CardSuit::Diamonds);
            return Some(Self::build_flush(sorted, flush_suit));
        }

        // --- Straight ---
        if let Some(high) = find_straight_high(all_rank_bits) {
            return Some(Self::build_straight(sorted, high, None));
        }

        // --- Three of a Kind ---
        if trips_count >= 1
            && let Some(tr) = trips_rank.first().copied().flatten()
        {
            return Some(Self::build_trips(sorted, tr));
        }

        // --- Two Pair ---
        if pair_count >= 2 {
            let p1 = pair_rank.first().copied().flatten();
            let p2 = pair_rank.get(1).copied().flatten();
            if let (Some(p1), Some(p2)) = (p1, p2) {
                return Some(Self::build_two_pair(sorted, p1, p2));
            }
        }

        // --- One Pair ---
        if pair_count == 1
            && let Some(p1) = pair_rank.first().copied().flatten()
        {
            return Some(Self::build_one_pair(sorted, p1));
        }

        // --- High Card ---
        Some(FullHand(
            sorted.first().copied()?,
            sorted.get(1).copied()?,
            sorted.get(2).copied()?,
            sorted.get(3).copied()?,
            sorted.get(4).copied()?,
        ))
    }

    // === Build helpers — each constructs the optimal FullHand for its type ===

    /// Build a straight (or straight flush if `suit` is Some).
    #[inline]
    fn build_straight(sorted: &[Card], high: CardNumber, suit: Option<CardSuit>) -> FullHand {
        let first = sorted
            .first()
            .copied()
            .unwrap_or(Card(CardNumber::Two, CardSuit::Diamonds));
        let mut result = [first; 5];
        let suit_filter = |c: &Card| suit.is_none_or(|s| c.suit() == s);
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
                if let Some(slot) = result.get_mut(i) {
                    *slot = sorted
                        .iter()
                        .find(|c| c.number() == r && suit_filter(c))
                        .copied()
                        .unwrap_or(first);
                }
            }
        } else {
            // high is one of 6..=14, so high-4..=high are all valid ranks.
            let hv = high.value();
            for i in 0..5u8 {
                // hv is 6..=14 and i is 0..5, so the subtraction can't underflow.
                let target_val = hv.checked_sub(i).unwrap_or(2);
                let target = rank_from_val(usize::from(target_val));
                if let Some(slot) = result.get_mut(usize::from(i)) {
                    *slot = sorted
                        .iter()
                        .find(|c| c.number() == target && suit_filter(c))
                        .copied()
                        .unwrap_or(first);
                }
            }
        }
        FullHand(result[0], result[1], result[2], result[3], result[4])
    }

    #[inline]
    fn build_quads(sorted: &[Card], quad_rank: CardNumber) -> FullHand {
        let first = sorted
            .first()
            .copied()
            .unwrap_or(Card(CardNumber::Two, CardSuit::Diamonds));
        let mut result = [first; 5];
        let mut idx = 0;
        for c in sorted.iter().filter(|c| c.number() == quad_rank) {
            if let Some(slot) = result.get_mut(idx) {
                *slot = *c;
            }
            idx = idx.saturating_add(1);
        }
        if let Some(c) = sorted.iter().find(|c| c.number() != quad_rank)
            && let Some(slot) = result.get_mut(idx)
        {
            *slot = *c;
        }
        FullHand(result[0], result[1], result[2], result[3], result[4])
    }

    #[inline]
    fn build_full_house(sorted: &[Card], trips: CardNumber, pair: CardNumber) -> FullHand {
        let first = sorted
            .first()
            .copied()
            .unwrap_or(Card(CardNumber::Two, CardSuit::Diamonds));
        let mut result = [first; 5];
        let mut idx = 0;
        for c in sorted.iter().filter(|c| c.number() == trips).take(3) {
            if let Some(slot) = result.get_mut(idx) {
                *slot = *c;
            }
            idx = idx.saturating_add(1);
        }
        for c in sorted.iter().filter(|c| c.number() == pair).take(2) {
            if let Some(slot) = result.get_mut(idx) {
                *slot = *c;
            }
            idx = idx.saturating_add(1);
        }
        FullHand(result[0], result[1], result[2], result[3], result[4])
    }

    #[inline]
    fn build_flush(sorted: &[Card], suit: CardSuit) -> FullHand {
        let first = sorted
            .first()
            .copied()
            .unwrap_or(Card(CardNumber::Two, CardSuit::Diamonds));
        let mut result = [first; 5];
        for (idx, c) in sorted
            .iter()
            .filter(|c| c.suit() == suit)
            .take(5)
            .enumerate()
        {
            if let Some(slot) = result.get_mut(idx) {
                *slot = *c;
            }
        }
        FullHand(result[0], result[1], result[2], result[3], result[4])
    }

    #[inline]
    fn build_trips(sorted: &[Card], trips: CardNumber) -> FullHand {
        let first = sorted
            .first()
            .copied()
            .unwrap_or(Card(CardNumber::Two, CardSuit::Diamonds));
        let mut result = [first; 5];
        let mut idx = 0;
        for c in sorted.iter().filter(|c| c.number() == trips).take(3) {
            if let Some(slot) = result.get_mut(idx) {
                *slot = *c;
            }
            idx = idx.saturating_add(1);
        }
        for c in sorted.iter().filter(|c| c.number() != trips).take(2) {
            if let Some(slot) = result.get_mut(idx) {
                *slot = *c;
            }
            idx = idx.saturating_add(1);
        }
        FullHand(result[0], result[1], result[2], result[3], result[4])
    }

    #[inline]
    fn build_two_pair(sorted: &[Card], p1: CardNumber, p2: CardNumber) -> FullHand {
        let first = sorted
            .first()
            .copied()
            .unwrap_or(Card(CardNumber::Two, CardSuit::Diamonds));
        let mut result = [first; 5];
        let mut idx = 0;
        for c in sorted.iter().filter(|c| c.number() == p1).take(2) {
            if let Some(slot) = result.get_mut(idx) {
                *slot = *c;
            }
            idx = idx.saturating_add(1);
        }
        for c in sorted.iter().filter(|c| c.number() == p2).take(2) {
            if let Some(slot) = result.get_mut(idx) {
                *slot = *c;
            }
            idx = idx.saturating_add(1);
        }
        for c in sorted
            .iter()
            .filter(|c| c.number() != p1 && c.number() != p2)
            .take(1)
        {
            if let Some(slot) = result.get_mut(idx) {
                *slot = *c;
            }
        }
        FullHand(result[0], result[1], result[2], result[3], result[4])
    }

    #[inline]
    fn build_one_pair(sorted: &[Card], pair: CardNumber) -> FullHand {
        let first = sorted
            .first()
            .copied()
            .unwrap_or(Card(CardNumber::Two, CardSuit::Diamonds));
        let mut result = [first; 5];
        let mut idx = 0;
        for c in sorted.iter().filter(|c| c.number() == pair).take(2) {
            if let Some(slot) = result.get_mut(idx) {
                *slot = *c;
            }
            idx = idx.saturating_add(1);
        }
        for c in sorted.iter().filter(|c| c.number() != pair).take(3) {
            if let Some(slot) = result.get_mut(idx) {
                *slot = *c;
            }
            idx = idx.saturating_add(1);
        }
        FullHand(result[0], result[1], result[2], result[3], result[4])
    }
}

/// Bitmask for the wheel straight (A-5-4-3-2).
const WHEEL_MASK: u16 = (1 << 14) | (1 << 5) | (1 << 4) | (1 << 3) | (1 << 2);
/// Bitmask for a royal flush (A-K-Q-J-T).
const ROYAL_MASK: u16 = (1 << 14) | (1 << 13) | (1 << 12) | (1 << 11) | (1 << 10);

/// Convert a `CardSuit` to a 0–3 index for array lookups.
#[inline]
const fn suit_ordinal(suit: CardSuit) -> usize {
    match suit {
        CardSuit::Diamonds => 0,
        CardSuit::Spades => 1,
        CardSuit::Clubs => 2,
        CardSuit::Hearts => 3,
    }
}

/// Convert a rank integer (2..=14) back to a `CardNumber`.
///
/// `val` must be in the range `2..=14`; out-of-range values clamp to the
/// nearest valid rank.
#[inline]
fn rank_from_val(val: usize) -> CardNumber {
    // val is always 2..=14 at the call sites; clamp defensively so the slice
    // access can't panic.
    let idx = val
        .saturating_sub(2)
        .min(CardNumber::ALL.len().saturating_sub(1));
    CardNumber::ALL.get(idx).copied().unwrap_or(CardNumber::Two)
}

/// Find the highest straight in a rank bitmask (bit *r* set ⇔ rank *r* present).
/// Returns the high card of the straight, or `CardNumber::Five` for a wheel.
#[inline]
fn find_straight_high(rank_bits: u16) -> Option<CardNumber> {
    // Check 5-wide windows from Ace-high (14) down to Six-high (6).
    for high in (6u8..=14).rev() {
        // high is 6..=14, so high-4 is 2..=10: no underflow.
        let shift = u32::from(high.checked_sub(4).unwrap_or(2));
        let mask: u16 = 0x1F_u16.checked_shl(shift).unwrap_or(0);
        if rank_bits & mask == mask {
            return Some(rank_from_val(usize::from(high)));
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
#[must_use]
pub const fn get_all_numbers() -> [CardNumber; 13] {
    CardNumber::ALL
}

/// All 52 cards, ordered by suit then rank.
// Const initializers can't use `.get_mut()` or `checked_*` in stable const
// eval, so the bounded indexing and incrementing here are allowed locally.
// `i` runs 0..52, `s` 0..4, `n` 0..13 — all provably in bounds by construction.
#[allow(clippy::indexing_slicing, clippy::arithmetic_side_effects)]
pub const ALL_CARDS: [Card; 52] = {
    let mut cards = [Card(CardNumber::Two, CardSuit::Diamonds); 52];
    let mut i = 0usize;
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
#[must_use]
pub const fn get_all_cards() -> [Card; 52] {
    ALL_CARDS
}

/// Dummy card used as a fallback when a deck position is unexpectedly empty.
const DUMMY_CARD: Card = Card(CardNumber::Two, CardSuit::Diamonds);

/// Draw `count` cards from `deck` starting at `offset`, returning them and the
/// advanced offset. Used by the Monte-Carlo equity simulators.
#[inline]
fn draw_cards(deck: &[Card], offset: usize, count: usize) -> ([Card; 3], usize) {
    let mut out = [DUMMY_CARD; 3];
    for k in 0..count {
        if let Some(slot) = out.get_mut(k) {
            *slot = deck
                .get(offset.saturating_add(k))
                .copied()
                .unwrap_or(DUMMY_CARD);
        }
    }
    (out, offset.saturating_add(count))
}

/// Build a simulated board, filling any missing stages from `deck` starting at
/// `offset`. Returns the completed board and the offset past the last drawn card.
#[inline]
fn build_sim_board(board: &Board, deck: &[Card], start: usize) -> (Board, usize) {
    let mut di = start;
    let sim_flop = board.flop.unwrap_or_else(|| {
        let (cards, next) = draw_cards(deck, di, 3);
        di = next;
        (cards[0], cards[1], cards[2])
    });
    let sim_turn = board.turn.unwrap_or_else(|| {
        let (cards, next) = draw_cards(deck, di, 1);
        di = next;
        cards[0]
    });
    let sim_river = board.river.unwrap_or_else(|| {
        let (cards, next) = draw_cards(deck, di, 1);
        di = next;
        cards[0]
    });
    (
        Board {
            flop: Some(sim_flop),
            turn: Some(sim_turn),
            river: Some(sim_river),
        },
        di,
    )
}

/// Given `(id, FullHand)` pairs, return the ids of the best hand(s). Ties
/// produce multiple ids.
///
/// Single-pass (O(n)): keeps the current best and replaces it only when a
/// strictly better hand is found. Shared by the equity simulator and the
/// engine's side-pot winner selection (`game_logic::find_pot_winners`).
pub fn best_hand_indices<T: Copy>(best_hands: &[(T, FullHand)]) -> Vec<T> {
    let mut winner_indices = Vec::new();
    let Some(&(first_idx, first_hand)) = best_hands.first() else {
        return winner_indices;
    };
    winner_indices.push(first_idx);
    let mut best = first_hand;

    for &(i, full_hand) in best_hands.iter().skip(1) {
        match full_hand.compare(&best) {
            Winner::Hand1 => {
                winner_indices.clear();
                winner_indices.push(i);
                best = full_hand;
            }
            Winner::Tie => {
                winner_indices.push(i);
            }
            Winner::Hand2 => {}
        }
    }
    winner_indices
}

/// Build a deck template with all `known` cards removed, returned as a
/// stack-allocated `[Card; 52]` plus the number of cards populated. Shared by
/// both Monte-Carlo equity simulators.
#[inline]
fn build_deck_template(known: &[Card]) -> ([Card; 52], usize) {
    let all_cards = get_all_cards();
    let mut deck_template = [DUMMY_CARD; 52];
    let mut deck_size = 0usize;
    for c in &all_cards {
        if !known.is_empty() && known.contains(c) {
            continue;
        }
        if let Some(slot) = deck_template.get_mut(deck_size) {
            *slot = *c;
        }
        deck_size = deck_size.saturating_add(1);
    }
    (deck_template, deck_size)
}

/// Partial Fisher-Yates: randomise only the first `n` positions of `deck`
/// (populated up to `deck_size`), leaving the rest untouched. Used by both
/// equity simulators to shuffle only the cards they'll actually deal.
#[inline]
fn partial_shuffle(deck: &mut [Card; 52], deck_size: usize, n: usize, rng: &mut impl RngExt) {
    for i in 0..n {
        let j = rng.random_range(i..deck_size);
        // i < j holds in Fisher-Yates; split the slice to get two mutable refs.
        let (left, right) = deck.split_at_mut(j);
        if let (Some(a), Some(b)) = (left.get_mut(i), right.first_mut()) {
            std::mem::swap(a, b);
        }
    }
}

/// Convert a win/tie/loss count (bounded by `iterations`, which fits u32) to an
/// f64 fraction without precision loss. Shared by both equity simulators.
#[inline]
fn to_pct(n: u64) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

#[allow(dead_code)]
#[must_use]
pub fn calculate_equity(hero: &Hand, board: &Board, iterations: usize) -> (f64, f64, f64) {
    let mut wins = 0u64;
    let mut ties = 0u64;
    let mut losses = 0u64;
    let mut rng = rng();

    // Identify known cards (stack-allocated)
    let mut known = [DUMMY_CARD; 7];
    if let Some(slot) = known.get_mut(0) {
        *slot = hero.0;
    }
    if let Some(slot) = known.get_mut(1) {
        *slot = hero.1;
    }
    let (bc, blen) = board.cards();
    // Hand occupies slots 0,1; board fills 2..(2+blen).
    let start: usize = 2;
    let end = start.saturating_add(blen).min(known.len());
    if end > start
        && let Some(dst) = known.get_mut(start..end)
        && let Some(src) = bc.get(..end.saturating_sub(start))
    {
        dst.copy_from_slice(src);
    }
    let known_len = start.saturating_add(blen);

    let (deck_template, deck_size) = build_deck_template(known.get(..known_len).unwrap_or(&[]));

    // How many random cards we need per iteration:
    // 2 (villain) + missing board cards
    let board_missing = 5usize.saturating_sub(blen);
    let cards_needed = 2usize.saturating_add(board_missing);

    for _ in 0..iterations {
        // Copy template (stack → stack, no heap)
        let mut deck = deck_template;

        partial_shuffle(&mut deck, deck_size, cards_needed, &mut rng);

        // Deal villain from first 2 positions
        let v0 = deck.first().copied().unwrap_or(DUMMY_CARD);
        let v1 = deck.get(1).copied().unwrap_or(DUMMY_CARD);
        let villain = Hand(v0, v1);

        // Build simulated board from positions 2..
        let (sim_board, _) = build_sim_board(board, &deck, 2);

        match determine_winner(hero, &villain, &sim_board) {
            Some(Winner::Hand1) => wins = wins.saturating_add(1),
            Some(Winner::Hand2) => losses = losses.saturating_add(1),
            Some(Winner::Tie) => ties = ties.saturating_add(1),
            None => {}
        }
    }

    let iter_f64 = f64::from(u32::try_from(iterations).unwrap_or(u32::MAX));
    (
        to_pct(wins) / iter_f64,
        to_pct(ties) / iter_f64,
        to_pct(losses) / iter_f64,
    )
}

/// Calculate equity for multiple hands in an all-in situation.
///
/// Returns a vector of equities (win% + tie%) for each hand in the same order.
#[must_use]
pub fn calculate_equity_multi(hands: &[Hand], board: &Board, iterations: usize) -> Vec<f64> {
    if hands.is_empty() {
        return vec![];
    }
    if hands.len() == 1 {
        return vec![100.0];
    }

    let num_hands = hands.len();
    let mut wins: Vec<u64> = vec![0; num_hands];
    let mut ties: Vec<u64> = vec![0; num_hands];
    let mut rng = rng();

    // Identify known cards (stack-allocated — max 23 cards: 9 players × 2 + 5 board)
    let mut known = [DUMMY_CARD; 23];
    let mut known_len = 0usize;
    for h in hands {
        if let Some(slot) = known.get_mut(known_len) {
            *slot = h.0;
        }
        if let Some(slot) = known.get_mut(known_len.saturating_add(1)) {
            *slot = h.1;
        }
        known_len = known_len.saturating_add(2);
    }
    let (bc, blen) = board.cards();
    let end = known_len.saturating_add(blen).min(known.len());
    if end > known_len
        && let Some(dst) = known.get_mut(known_len..end)
        && let Some(src) = bc.get(..end.saturating_sub(known_len))
    {
        dst.copy_from_slice(src);
    }
    known_len = known_len.saturating_add(blen);

    let (deck_template, deck_size) = build_deck_template(known.get(..known_len).unwrap_or(&[]));

    // Only need to deal missing board cards (all player hands are known)
    let board_missing = 5usize.saturating_sub(blen);

    // Pre-allocate per-iteration scratch (avoids Vec allocation in the hot loop)
    let mut best_hands: Vec<(usize, FullHand)> = Vec::with_capacity(num_hands);
    let mut winner_indices: Vec<usize>;

    let iter_f64 = f64::from(u32::try_from(iterations).unwrap_or(u32::MAX));

    for _ in 0..iterations {
        let mut deck = deck_template;

        partial_shuffle(&mut deck, deck_size, board_missing, &mut rng);

        // Build simulated board
        let (sim_board, _) = build_sim_board(board, &deck, 0);

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
        winner_indices = best_hand_indices(&best_hands);

        if winner_indices.len() == 1
            && let Some(&idx) = winner_indices.first()
            && let Some(slot) = wins.get_mut(idx)
        {
            *slot = slot.saturating_add(1);
        } else {
            for &idx in &winner_indices {
                if let Some(slot) = ties.get_mut(idx) {
                    *slot = slot.saturating_add(1);
                }
            }
        }
    }

    // Calculate equity as win% + (tie% / number_of_tiers). Counts are bounded
    // by `iterations` (which fits u32), so narrowing before the f64 conversion is lossless.
    hands
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let win_pct = (to_pct(wins.get(i).copied().unwrap_or(0)) / iter_f64) * 100.0;
            let tie_pct = (to_pct(ties.get(i).copied().unwrap_or(0)) / iter_f64) * 100.0;
            // For ties, equity is split proportionally among tied players
            // Approximate by dividing tie equity by average number of players in ties
            win_pct + (tie_pct / 2.0)
        })
        .collect()
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{c, make_board};

    #[test]
    fn test_card_display() {
        let card = c(CardNumber::Ace, CardSuit::Spades);
        assert_eq!(format!("{card}"), "A♠");

        let card = c(CardNumber::Ten, CardSuit::Hearts);
        assert_eq!(format!("{card}"), "10♥");

        let card = c(CardNumber::Two, CardSuit::Diamonds);
        assert_eq!(format!("{card}"), "2♦");
    }

    #[test]
    fn card_suit_is_red() {
        assert!(CardSuit::Diamonds.is_red());
        assert!(CardSuit::Hearts.is_red());
        assert!(!CardSuit::Spades.is_red());
        assert!(!CardSuit::Clubs.is_red());
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
            assert_eq!(count, 4, "Should have 4 cards of {number:?}");
        }

        // Check we have 13 of each suit
        for suit in CardSuit::ALL {
            let count = cards.iter().filter(|c| c.suit() == suit).count();
            assert_eq!(count, 13, "Should have 13 cards of {suit:?}");
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
