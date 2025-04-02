# Solana Poker Game Smart Contract Documentation

This document provides a comprehensive overview of the Solana-based poker game smart contract. The contract enables on-chain Texas Hold'em poker games with token-based buy-ins, betting, and payouts.

## Overview

The poker game contract is built using the Anchor framework for Solana and implements a complete Texas Hold'em poker game with the following features:

- Creating and joining poker tables
- Buy-ins with SPL tokens
- Betting rounds (pre-flop, flop, turn, river)
- Player actions (check, bet, call, fold)
- Hand evaluation and pot distribution
- Table management (joining, leaving, resetting)

## Account Structure

The contract uses several account types to manage the game state:

### GameAuthority

Central authority account that tracks global game statistics and settings.

```rust
pub struct GameAuthority {
    pub authority: Pubkey,        // Admin address
    pub fee_percentage: u8,       // Fee percentage (0-10%)
    pub total_games_played: u64,  // Total number of games played
    pub total_fees_collected: u64, // Total fees collected
    pub bump: u8,                 // PDA bump
}

