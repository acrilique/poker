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

use rand::rng;
use rand::seq::SliceRandom;
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
    /// Collect all community cards into a Vec
    pub fn cards(&self) -> Vec<Card> {
        let mut cards = Vec::new();
        if let Some((c1, c2, c3)) = self.flop {
            cards.extend_from_slice(&[c1, c2, c3]);
        }
        if let Some(c) = self.turn {
            cards.push(c);
        }
        if let Some(c) = self.river {
            cards.push(c);
        }
        cards
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
    /// Get all cards as a sorted vector (highest first)
    fn cards(&self) -> Vec<Card> {
        let mut cards = vec![self.0, self.1, self.2, self.3, self.4];
        cards.sort_by_key(|c| std::cmp::Reverse(c.number()));
        cards
    }

    /// Determine the rank of this hand
    pub fn rank(&self) -> HandRank {
        let cards = self.cards();
        let is_flush = cards.iter().all(|c| c.suit() == cards[0].suit());
        let is_straight = is_consecutive(&cards.iter().map(|c| c.number()).collect::<Vec<_>>())
            || self.is_wheel();
        let counts = self.get_counts();

        match (is_flush, is_straight, &counts[..]) {
            (true, true, _)
                if cards[0].number() == CardNumber::Ace
                    && cards[1].number() == CardNumber::King =>
            {
                HandRank::RoyalFlush
            }
            (true, true, _) => HandRank::StraightFlush,
            (_, _, [4, 1]) => HandRank::FourOfAKind,
            (_, _, [3, 2]) => HandRank::FullHouse,
            (true, false, _) => HandRank::Flush,
            (false, true, _) => HandRank::Straight,
            (_, _, [3, 1, 1]) => HandRank::ThreeOfAKind,
            (_, _, [2, 2, 1]) => HandRank::TwoPair,
            (_, _, [2, 1, 1, 1]) => HandRank::Pair,
            _ => HandRank::HighCard,
        }
    }

    /// Check if this is a wheel (A-2-3-4-5)
    fn is_wheel(&self) -> bool {
        let cards = self.cards();
        let numbers: Vec<CardNumber> = cards.iter().map(|c| c.number()).collect();
        numbers.contains(&CardNumber::Ace)
            && numbers.contains(&CardNumber::Two)
            && numbers.contains(&CardNumber::Three)
            && numbers.contains(&CardNumber::Four)
            && numbers.contains(&CardNumber::Five)
    }

    /// Get counts of each rank, sorted descending
    fn get_counts(&self) -> Vec<usize> {
        let cards = self.cards();
        let mut counts: Vec<usize> = Vec::new();
        let mut seen: Vec<CardNumber> = Vec::new();

        for card in &cards {
            if !seen.contains(&card.number()) {
                seen.push(card.number());
                let count = cards.iter().filter(|c| c.number() == card.number()).count();
                counts.push(count);
            }
        }
        counts.sort_by(|a, b| b.cmp(a));
        counts
    }

    /// Get card numbers grouped by count (for tiebreakers)
    /// Returns (groups_by_count, kickers) where groups are sorted by count desc, then rank desc
    fn get_ranked_groups(&self) -> Vec<CardNumber> {
        let cards = self.cards();
        let mut groups: Vec<(usize, CardNumber)> = Vec::new();
        let mut seen: Vec<CardNumber> = Vec::new();

        for card in &cards {
            if !seen.contains(&card.number()) {
                seen.push(card.number());
                let count = cards.iter().filter(|c| c.number() == card.number()).count();
                groups.push((count, card.number()));
            }
        }
        // Sort by count descending, then by rank descending
        groups.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        groups.into_iter().map(|(_, n)| n).collect()
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
    /// Collects all available cards (hand + board) into a Vec
    fn all_cards(&self, board: &Board) -> Vec<Card> {
        let mut cards = vec![self.0, self.1];
        cards.extend(board.cards());
        cards
    }

    /// Returns the best possible 5-card hand
    pub fn best(&self, board: &Board) -> Option<FullHand> {
        self.try_royal_flush(board)
            .or_else(|| self.try_straight_flush(board))
            .or_else(|| self.try_poker(board))
            .or_else(|| self.try_full_house(board))
            .or_else(|| self.try_flush(board))
            .or_else(|| self.try_straight(board))
            .or_else(|| self.try_trio(board))
            .or_else(|| self.try_doubles(board))
            .or_else(|| self.try_pair(board))
            .or_else(|| self.try_high_card(board))
    }

    /// Royal Flush: A, K, Q, J, 10 of the same suit
    fn try_royal_flush(&self, board: &Board) -> Option<FullHand> {
        let cards = self.all_cards(board);
        for suit in CardSuit::ALL {
            let ace = cards
                .iter()
                .find(|c| c.number() == CardNumber::Ace && c.suit() == suit);
            let king = cards
                .iter()
                .find(|c| c.number() == CardNumber::King && c.suit() == suit);
            let queen = cards
                .iter()
                .find(|c| c.number() == CardNumber::Queen && c.suit() == suit);
            let jack = cards
                .iter()
                .find(|c| c.number() == CardNumber::Jack && c.suit() == suit);
            let ten = cards
                .iter()
                .find(|c| c.number() == CardNumber::Ten && c.suit() == suit);

            if let (Some(&a), Some(&k), Some(&q), Some(&j), Some(&t)) =
                (ace, king, queen, jack, ten)
            {
                return Some(FullHand(a, k, q, j, t));
            }
        }
        None
    }

    /// Straight Flush: 5 consecutive cards of the same suit
    fn try_straight_flush(&self, board: &Board) -> Option<FullHand> {
        let cards = self.all_cards(board);
        for suit in CardSuit::ALL {
            let mut suited: Vec<Card> =
                cards.iter().filter(|c| c.suit() == suit).copied().collect();
            suited.sort_by_key(|c| std::cmp::Reverse(c.number()));
            suited.dedup_by(|a, b| a.number() == b.number());

            if suited.len() >= 5 {
                for window in suited.windows(5) {
                    if is_consecutive(&window.iter().map(|c| c.number()).collect::<Vec<_>>()) {
                        return Some(FullHand(
                            window[0], window[1], window[2], window[3], window[4],
                        ));
                    }
                }
                // Check for wheel (A-2-3-4-5)
                let has_ace = suited.iter().any(|c| c.number() == CardNumber::Ace);
                let wheel: Vec<Card> = suited
                    .iter()
                    .filter(|c| {
                        matches!(
                            c.number(),
                            CardNumber::Two
                                | CardNumber::Three
                                | CardNumber::Four
                                | CardNumber::Five
                        )
                    })
                    .copied()
                    .collect();
                if has_ace && wheel.len() == 4 {
                    let ace = suited
                        .iter()
                        .find(|c| c.number() == CardNumber::Ace)
                        .unwrap();
                    return Some(FullHand(wheel[0], wheel[1], wheel[2], wheel[3], *ace));
                }
            }
        }
        None
    }

    /// Four of a Kind (Poker): 4 cards of the same rank
    fn try_poker(&self, board: &Board) -> Option<FullHand> {
        let cards = self.all_cards(board);
        let mut sorted = cards.clone();
        sorted.sort_by_key(|c| std::cmp::Reverse(c.number()));

        for num in get_all_numbers().into_iter().rev() {
            let quads: Vec<Card> = sorted
                .iter()
                .filter(|c| c.number() == num)
                .copied()
                .collect();
            if quads.len() == 4 {
                let kicker = sorted.iter().find(|c| c.number() != num).copied()?;
                return Some(FullHand(quads[0], quads[1], quads[2], quads[3], kicker));
            }
        }
        None
    }

    /// Full House: 3 of a kind + a pair
    fn try_full_house(&self, board: &Board) -> Option<FullHand> {
        let cards = self.all_cards(board);
        let mut sorted = cards.clone();
        sorted.sort_by_key(|c| std::cmp::Reverse(c.number()));

        for trio_num in get_all_numbers().into_iter().rev() {
            let trio: Vec<Card> = sorted
                .iter()
                .filter(|c| c.number() == trio_num)
                .copied()
                .collect();
            if trio.len() >= 3 {
                for pair_num in get_all_numbers().into_iter().rev() {
                    if pair_num == trio_num {
                        continue;
                    }
                    let pair: Vec<Card> = sorted
                        .iter()
                        .filter(|c| c.number() == pair_num)
                        .copied()
                        .collect();
                    if pair.len() >= 2 {
                        return Some(FullHand(trio[0], trio[1], trio[2], pair[0], pair[1]));
                    }
                }
            }
        }
        None
    }

    /// Flush (Color): 5 cards of the same suit
    fn try_flush(&self, board: &Board) -> Option<FullHand> {
        let cards = self.all_cards(board);
        for suit in CardSuit::ALL {
            let mut suited: Vec<Card> =
                cards.iter().filter(|c| c.suit() == suit).copied().collect();
            if suited.len() >= 5 {
                suited.sort_by_key(|c| std::cmp::Reverse(c.number()));
                return Some(FullHand(
                    suited[0], suited[1], suited[2], suited[3], suited[4],
                ));
            }
        }
        None
    }

    /// Straight: 5 consecutive cards
    fn try_straight(&self, board: &Board) -> Option<FullHand> {
        let cards = self.all_cards(board);
        let mut sorted = cards.clone();
        sorted.sort_by_key(|c| std::cmp::Reverse(c.number()));
        sorted.dedup_by(|a, b| a.number() == b.number());

        if sorted.len() >= 5 {
            for window in sorted.windows(5) {
                if is_consecutive(&window.iter().map(|c| c.number()).collect::<Vec<_>>()) {
                    return Some(FullHand(
                        window[0], window[1], window[2], window[3], window[4],
                    ));
                }
            }
            // Check for wheel (A-2-3-4-5)
            let has_ace = sorted.iter().any(|c| c.number() == CardNumber::Ace);
            let wheel: Vec<Card> = sorted
                .iter()
                .filter(|c| {
                    matches!(
                        c.number(),
                        CardNumber::Two | CardNumber::Three | CardNumber::Four | CardNumber::Five
                    )
                })
                .copied()
                .collect();
            if has_ace && wheel.len() == 4 {
                let ace = sorted
                    .iter()
                    .find(|c| c.number() == CardNumber::Ace)
                    .unwrap();
                return Some(FullHand(wheel[0], wheel[1], wheel[2], wheel[3], *ace));
            }
        }
        None
    }

    /// Three of a Kind (Trio): 3 cards of the same rank
    fn try_trio(&self, board: &Board) -> Option<FullHand> {
        let cards = self.all_cards(board);
        let mut sorted = cards.clone();
        sorted.sort_by_key(|c| std::cmp::Reverse(c.number()));

        for num in get_all_numbers().into_iter().rev() {
            let trio: Vec<Card> = sorted
                .iter()
                .filter(|c| c.number() == num)
                .copied()
                .collect();
            if trio.len() == 3 {
                let kickers: Vec<Card> = sorted
                    .iter()
                    .filter(|c| c.number() != num)
                    .copied()
                    .take(2)
                    .collect();
                if kickers.len() >= 2 {
                    return Some(FullHand(trio[0], trio[1], trio[2], kickers[0], kickers[1]));
                }
            }
        }
        None
    }

    /// Two Pair: 2 different pairs
    fn try_doubles(&self, board: &Board) -> Option<FullHand> {
        let cards = self.all_cards(board);
        let mut sorted = cards.clone();
        sorted.sort_by_key(|c| std::cmp::Reverse(c.number()));

        let mut pairs: Vec<Vec<Card>> = Vec::new();
        for num in get_all_numbers().into_iter().rev() {
            let pair: Vec<Card> = sorted
                .iter()
                .filter(|c| c.number() == num)
                .copied()
                .collect();
            if pair.len() >= 2 {
                pairs.push(pair);
            }
        }

        if pairs.len() >= 2 {
            let kicker = sorted
                .iter()
                .find(|c| c.number() != pairs[0][0].number() && c.number() != pairs[1][0].number())
                .copied()?;
            return Some(FullHand(
                pairs[0][0],
                pairs[0][1],
                pairs[1][0],
                pairs[1][1],
                kicker,
            ));
        }
        None
    }

    /// One Pair: 2 cards of the same rank
    fn try_pair(&self, board: &Board) -> Option<FullHand> {
        let cards = self.all_cards(board);
        let mut sorted = cards.clone();
        sorted.sort_by_key(|c| std::cmp::Reverse(c.number()));

        for num in get_all_numbers().into_iter().rev() {
            let pair: Vec<Card> = sorted
                .iter()
                .filter(|c| c.number() == num)
                .copied()
                .collect();
            if pair.len() >= 2 {
                let kickers: Vec<Card> = sorted
                    .iter()
                    .filter(|c| c.number() != num)
                    .copied()
                    .take(3)
                    .collect();
                if kickers.len() >= 3 {
                    return Some(FullHand(
                        pair[0], pair[1], kickers[0], kickers[1], kickers[2],
                    ));
                }
            }
        }
        None
    }

    /// High Card: Best 5 cards when no other hand is made
    fn try_high_card(&self, board: &Board) -> Option<FullHand> {
        let cards = self.all_cards(board);
        if cards.len() < 5 {
            return None;
        }
        let mut sorted = cards.clone();
        sorted.sort_by_key(|c| std::cmp::Reverse(c.number()));
        Some(FullHand(
            sorted[0], sorted[1], sorted[2], sorted[3], sorted[4],
        ))
    }
}

/// Helper function to check if card numbers are consecutive
fn is_consecutive(numbers: &[CardNumber]) -> bool {
    if numbers.len() < 2 {
        return true;
    }
    for i in 0..numbers.len() - 1 {
        if (numbers[i] as i32) - (numbers[i + 1] as i32) != 1 {
            return false;
        }
    }
    true
}

/// Helper function to get all card numbers
pub fn get_all_numbers() -> Vec<CardNumber> {
    vec![
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
    ]
}

pub fn get_all_cards() -> Vec<Card> {
    let mut cards = Vec::new();
    for suit in CardSuit::ALL {
        for number in get_all_numbers() {
            cards.push(Card(number, suit));
        }
    }
    cards
}

#[allow(dead_code)]
pub fn calculate_equity(hero: &Hand, board: &Board, iterations: usize) -> (f64, f64, f64) {
    let mut wins = 0;
    let mut ties = 0;
    let mut losses = 0;
    let mut rng = rng();

    // Identify known cards
    let mut known_cards = vec![hero.0, hero.1];
    known_cards.extend(board.cards());

    let all_cards = get_all_cards();

    for _ in 0..iterations {
        // Create deck excluding known cards
        let mut deck: Vec<Card> = all_cards
            .iter()
            .filter(|c| !known_cards.contains(c))
            .copied()
            .collect();

        deck.shuffle(&mut rng);

        // Deal villain
        let v1 = deck.pop().unwrap();
        let v2 = deck.pop().unwrap();
        let villain = Hand(v1, v2);

        // Fill board
        let sim_board = board.fill_from_deck(&mut deck);

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

    // Identify known cards (all hands + board)
    let mut known_cards: Vec<Card> = hands.iter().flat_map(|h| [h.0, h.1]).collect();
    known_cards.extend(board.cards());

    let all_cards = get_all_cards();

    for _ in 0..iterations {
        // Create deck excluding known cards
        let mut deck: Vec<Card> = all_cards
            .iter()
            .filter(|c| !known_cards.contains(c))
            .copied()
            .collect();

        deck.shuffle(&mut rng);

        // Fill board
        let sim_board = board.fill_from_deck(&mut deck);

        // Evaluate all hands and find winner(s)
        let mut best_hands: Vec<(usize, FullHand)> = Vec::new();
        for (i, hand) in hands.iter().enumerate() {
            if let Some(full_hand) = hand.best(&sim_board) {
                best_hands.push((i, full_hand));
            }
        }

        if best_hands.is_empty() {
            continue;
        }

        // Find the best hand(s)
        let mut winner_indices: Vec<usize> = vec![best_hands[0].0];
        let mut best = &best_hands[0].1;

        for (i, full_hand) in best_hands.iter().skip(1) {
            match full_hand.compare(best) {
                Winner::Hand1 => {
                    // This hand is better
                    winner_indices.clear();
                    winner_indices.push(*i);
                    best = full_hand;
                }
                Winner::Tie => {
                    // Tie with current best
                    winner_indices.push(*i);
                }
                Winner::Hand2 => {
                    // Current best is still better
                }
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
