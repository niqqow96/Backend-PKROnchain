use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};
use std::collections::HashMap;

declare_id!("Poker11111111111111111111111111111111111111");

#[program]
pub mod poker_game {
    use super::*;

    /// Initialize the poker game program with global settings
    pub fn initialize(ctx: Context<Initialize>, fee_percentage: u8) -> Result<()> {
        require!(fee_percentage <= 10, ErrorCode::FeeTooHigh); // Max 10% fee

        let game_authority = &mut ctx.accounts.game_authority;
        game_authority.authority = ctx.accounts.authority.key();
        game_authority.fee_percentage = fee_percentage;
        game_authority.total_games_played = 0;
        game_authority.total_fees_collected = 0;
        game_authority.bump = *ctx.bumps.get("game_authority").unwrap();

        Ok(())
    }

    /// Create a new poker table with specified parameters
    pub fn create_table(
        ctx: Context<CreateTable>,
        table_id: String,
        buy_in: u64,
        small_blind: u64,
        big_blind: u64,
        max_players: u8,
        is_private: bool,
    ) -> Result<()> {
        require!(max_players >= 2 && max_players <= 9, ErrorCode::InvalidPlayerCount);
        require!(big_blind >= small_blind, ErrorCode::InvalidBlinds);
        require!(buy_in >= big_blind * 10, ErrorCode::BuyInTooSmall);
        require!(table_id.len() <= 32, ErrorCode::TableIdTooLong);

        let table = &mut ctx.accounts.table;
        table.host = ctx.accounts.host.key();
        table.table_id = table_id;
        table.buy_in = buy_in;
        table.small_blind = small_blind;
        table.big_blind = big_blind;
        table.max_players = max_players;
        table.is_private = is_private;
        table.status = TableStatus::Waiting;
        table.pot = 0;
        table.current_player_index = 0;
        table.dealer_index = 0;
        table.round = Round::NotStarted;
        table.player_count = 0;
        table.bump = *ctx.bumps.get("table").unwrap();
        
        // Initialize empty player slots
        table.players = vec![Pubkey::default(); max_players as usize];
        
        // Add host as first player
        table.players[0] = ctx.accounts.host.key();
        table.player_count = 1;

        // Transfer buy-in from host to table vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.host_token_account.to_account_info(),
            to: ctx.accounts.table_vault.to_account_info(),
            authority: ctx.accounts.host.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, buy_in)?;

        // Create player state for host
        let player_state = &mut ctx.accounts.player_state;
        player_state.player = ctx.accounts.host.key();
        player_state.table = ctx.accounts.table.key();
        player_state.chips = buy_in;
        player_state.is_active = true;
        player_state.is_folded = false;
        player_state.current_bet = 0;
        player_state.cards = [0, 0]; // Will be set when game starts
        player_state.bump = *ctx.bumps.get("player_state").unwrap();

        // Update game authority stats
        let game_authority = &mut ctx.accounts.game_authority;
        game_authority.total_games_played = game_authority.total_games_played.checked_add(1).unwrap();

        Ok(())
    }

    /// Join an existing poker table
    pub fn join_table(ctx: Context<JoinTable>) -> Result<()> {
        let table = &mut ctx.accounts.table;
        
        // Validate table state
        require!(table.status == TableStatus::Waiting, ErrorCode::TableNotWaiting);
        require!(table.player_count < table.max_players, ErrorCode::TableFull);
        
        // Find empty slot
        let mut slot_index = table.max_players as usize;
        for (i, player) in table.players.iter().enumerate() {
            if *player == Pubkey::default() {
                slot_index = i;
                break;
            }
        }
        require!(slot_index < table.max_players as usize, ErrorCode::TableFull);
        
        // Add player to table
        table.players[slot_index] = ctx.accounts.player.key();
        table.player_count = table.player_count.checked_add(1).unwrap();
        
        // Transfer buy-in from player to table vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.player_token_account.to_account_info(),
            to: ctx.accounts.table_vault.to_account_info(),
            authority: ctx.accounts.player.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, table.buy_in)?;
        
        // Create player state
        let player_state = &mut ctx.accounts.player_state;
        player_state.player = ctx.accounts.player.key();
        player_state.table = ctx.accounts.table.key();
        player_state.chips = table.buy_in;
        player_state.is_active = true;
        player_state.is_folded = false;
        player_state.current_bet = 0;
        player_state.cards = [0, 0]; // Will be set when game starts
        player_state.bump = *ctx.bumps.get("player_state").unwrap();
        
        Ok(())
    }

    /// Start a poker game on a table that has enough players
    pub fn start_game(ctx: Context<StartGame>, seed: u64) -> Result<()> {
        let table = &mut ctx.accounts.table;
        
        // Validate table state
        require!(table.status == TableStatus::Waiting, ErrorCode::TableNotWaiting);
        require!(table.player_count >= 2, ErrorCode::NotEnoughPlayers);
        require!(ctx.accounts.host.key() == table.host, ErrorCode::NotTableHost);
        
        // Update table status
        table.status = TableStatus::Playing;
        table.round = Round::PreFlop;
        
        // Set dealer position (can be randomized based on seed)
        table.dealer_index = (seed % table.player_count as u64) as u8;
        
        // Calculate small blind and big blind positions
        let sb_index = (table.dealer_index + 1) % table.player_count;
        let bb_index = (table.dealer_index + 2) % table.player_count;
        
        // Set current player to the one after big blind
        table.current_player_index = (bb_index + 1) % table.player_count;
        
        // Deal cards to players (in a real implementation, this would use a verifiable random function)
        // For now, we'll use a simple deterministic approach based on the seed
        let mut deck = generate_shuffled_deck(seed);
        
        // Deal two cards to each active player
        let mut card_index = 0;
        for (i, player_pubkey) in table.players.iter().enumerate() {
            if *player_pubkey != Pubkey::default() {
                // Find player state account
                let seeds = &[
                    b"player_state".as_ref(),
                    player_pubkey.as_ref(),
                    table.key().as_ref(),
                    &[ctx.accounts.player_states[i].bump],
                ];
                let player_state = &mut ctx.accounts.player_states[i];
                
                // Deal two cards to this player
                player_state.cards = [deck[card_index], deck[card_index + 1]];
                card_index += 2;
            }
        }
        
        // Store community cards for later reveals
        table.community_cards = [
            deck[card_index],     // flop 1
            deck[card_index + 1], // flop 2
            deck[card_index + 2], // flop 3
            deck[card_index + 3], // turn
            deck[card_index + 4], // river
        ];
        
        // Post blinds
        let sb_player = &mut ctx.accounts.player_states[sb_index as usize];
        let bb_player = &mut ctx.accounts.player_states[bb_index as usize];
        
        // Small blind
        sb_player.current_bet = table.small_blind;
        sb_player.chips = sb_player.chips.checked_sub(table.small_blind).unwrap();
        
        // Big blind
        bb_player.current_bet = table.big_blind;
        bb_player.chips = bb_player.chips.checked_sub(table.big_blind).unwrap();
        
        // Update pot
        table.pot = table.small_blind.checked_add(table.big_blind).unwrap();
        
        // Initialize game state
        table.highest_bet = table.big_blind;
        
        Ok(())
    }

    /// Player makes a bet or raise
    pub fn bet(ctx: Context<PlayerAction>, amount: u64) -> Result<()> {
        let table = &mut ctx.accounts.table;
        let player_state = &mut ctx.accounts.player_state;
        
        // Validate table and player state
        require!(table.status == TableStatus::Playing, ErrorCode::GameNotInProgress);
        require!(!player_state.is_folded, ErrorCode::PlayerFolded);
        require!(player_state.is_active, ErrorCode::PlayerNotActive);
        
        // Verify it's this player's turn
        let current_player_pubkey = table.players[table.current_player_index as usize];
        require!(current_player_pubkey == ctx.accounts.player.key(), ErrorCode::NotPlayerTurn);
        
        // Calculate how much more the player needs to bet
        let additional_bet = amount.checked_sub(player_state.current_bet).unwrap();
        
        // Verify player has enough chips
        require!(player_state.chips >= additional_bet, ErrorCode::InsufficientChips);
        
        // Verify bet is at least the minimum raise
        if amount > table.highest_bet {
            let min_raise = table.highest_bet.checked_add(table.big_blind).unwrap();
            require!(amount >= min_raise, ErrorCode::BetTooSmall);
        } else {
            require!(amount == table.highest_bet, ErrorCode::BetTooSmall);
        }
        
        // Update player state
        player_state.chips = player_state.chips.checked_sub(additional_bet).unwrap();
        player_state.current_bet = amount;
        
        // Update table state
        table.pot = table.pot.checked_add(additional_bet).unwrap();
        if amount > table.highest_bet {
            table.highest_bet = amount;
        }
        
        // Move to next player
        advance_to_next_player(table)?;
        
        // Check if round is complete
        check_round_completion(ctx)?;
        
        Ok(())
    }

    /// Player checks (bet 0 when no previous bets)
    pub fn check(ctx: Context<PlayerAction>) -> Result<()> {
        let table = &mut ctx.accounts.table;
        let player_state = &mut ctx.accounts.player_state;
        
        // Validate table and player state
        require!(table.status == TableStatus::Playing, ErrorCode::GameNotInProgress);
        require!(!player_state.is_folded, ErrorCode::PlayerFolded);
        require!(player_state.is_active, ErrorCode::PlayerNotActive);
        
        // Verify it's this player's turn
        let current_player_pubkey = table.players[table.current_player_index as usize];
        require!(current_player_pubkey == ctx.accounts.player.key(), ErrorCode::NotPlayerTurn);
        
        // Can only check if no one has bet or player has matched the highest bet
        require!(table.highest_bet == 0 || player_state.current_bet == table.highest_bet, ErrorCode::CannotCheck);
        
        // Move to next player
        advance_to_next_player(table)?;
        
        // Check if round is complete
        check_round_completion(ctx)?;
        
        Ok(())
    }

    /// Player calls the current highest bet
    pub fn call(ctx: Context<PlayerAction>) -> Result<()> {
        let table = &mut ctx.accounts.table;
        let player_state = &mut ctx.accounts.player_state;
        
        // Validate table and player state
        require!(table.status == TableStatus::Playing, ErrorCode::GameNotInProgress);
        require!(!player_state.is_folded, ErrorCode::PlayerFolded);
        require!(player_state.is_active, ErrorCode::PlayerNotActive);
        
        // Verify it's this player's turn
        let current_player_pubkey = table.players[table.current_player_index as usize];
        require!(current_player_pubkey == ctx.accounts.player.key(), ErrorCode::NotPlayerTurn);
        
        // Calculate call amount
        let call_amount = table.highest_bet.checked_sub(player_state.current_bet).unwrap();
        
        // Handle all-in if player doesn't have enough chips
        let actual_call = std::cmp::min(call_amount, player_state.chips);
        
        // Update player state
        player_state.chips = player_state.chips.checked_sub(actual_call).unwrap();
        player_state.current_bet = player_state.current_bet.checked_add(actual_call).unwrap();
        
        // If player couldn't match the full bet, they're all-in
        if actual_call < call_amount {
            player_state.is_all_in = true;
        }
        
        // Update table state
        table.pot = table.pot.checked_add(actual_call).unwrap();
        
        // Move to next player
        advance_to_next_player(table)?;
        
        // Check if round is complete
        check_round_completion(ctx)?;
        
        Ok(())
    }

    /// Player folds their hand
    pub fn fold(ctx: Context<PlayerAction>) -> Result<()> {
        let table = &mut ctx.accounts.table;
        let player_state = &mut ctx.accounts.player_state;
        
        // Validate table and player state
        require!(table.status == TableStatus::Playing, ErrorCode::GameNotInProgress);
        require!(!player_state.is_folded, ErrorCode::PlayerFolded);
        require!(player_state.is_active, ErrorCode::PlayerNotActive);
        
        // Verify it's this player's turn
        let current_player_pubkey = table.players[table.current_player_index as usize];
        require!(current_player_pubkey == ctx.accounts.player.key(), ErrorCode::NotPlayerTurn);
        
        // Update player state
        player_state.is_folded = true;
        
        // Move to next player
        advance_to_next_player(table)?;
        
        // Check if only one player remains
        let active_players = count_active_players(ctx);
        if active_players == 1 {
            // Find the winner and award the pot
            for player_state in ctx.accounts.player_states.iter_mut() {
                if player_state.is_active && !player_state.is_folded {
                    player_state.chips = player_state.chips.checked_add(table.pot).unwrap();
                    break;
                }
            }
            
            // End the game
            table.status = TableStatus::Finished;
            return Ok(());
        }
        
        // Check if round is complete
        check_round_completion(ctx)?;
        
        Ok(())
    }

    /// Determine winner and distribute pot at showdown
    pub fn showdown(ctx: Context<Showdown>) -> Result<()> {
        let table = &mut ctx.accounts.table;
        
        // Validate table state
        require!(table.status == TableStatus::Playing, ErrorCode::GameNotInProgress);
        require!(table.round == Round::Showdown, ErrorCode::NotShowdownRound);
        
        // Calculate hand strengths for all active players
        let mut best_hand_value = 0;
        let mut winners = Vec::new();
        
        for (i, player_pubkey) in table.players.iter().enumerate() {
            if *player_pubkey == Pubkey::default() {
                continue;
            }
            
            let player_state = &ctx.accounts.player_states[i];
            if player_state.is_folded || !player_state.is_active {
                continue;
            }
            
            // Combine player's hole cards with community cards
            let mut cards = Vec::with_capacity(7);
            cards.push(player_state.cards[0]);
            cards.push(player_state.cards[1]);
            for &card in table.community_cards.iter() {
                cards.push(card);
            }
            
            // Evaluate hand strength
            let hand_value = evaluate_poker_hand(&cards);
            
            if hand_value > best_hand_value {
                best_hand_value = hand_value;
                winners.clear();
                winners.push(i);
            } else if hand_value == best_hand_value {
                winners.push(i);
            }
        }
        
        // Distribute pot among winners
        let winner_share = table.pot / winners.len() as u64;
        for &winner_index in winners.iter() {
            let winner_state = &mut ctx.accounts.player_states[winner_index];
            winner_state.chips = winner_state.chips.checked_add(winner_share).unwrap();
        }
        
        // Handle remainder chips (give to first winner)
        let remainder = table.pot % winners.len() as u64;
        if remainder > 0 {
            let first_winner = &mut ctx.accounts.player_states[winners[0]];
            first_winner.chips = first_winner.chips.checked_add(remainder).unwrap();
        }
        
        // End the game
        table.status = TableStatus::Finished;
        
        Ok(())
    }

    /// Reset the table for a new game
    pub fn reset_table(ctx: Context<ResetTable>) -> Result<()> {
        let table = &mut ctx.accounts.table;
        
        // Validate table state
        require!(table.status == TableStatus::Finished, ErrorCode::GameNotFinished);
        require!(ctx.accounts.host.key() == table.host, ErrorCode::NotTableHost);
        
        // Reset table state
        table.status = TableStatus::Waiting;
        table.pot = 0;
        table.round = Round::NotStarted;
        table.highest_bet = 0;
        
        // Reset player states
        for player_state in ctx.accounts.player_states.iter_mut() {
            if player_state.is_active {
                player_state.is_folded = false;
                player_state.current_bet = 0;
                player_state.is_all_in = false;
            }
        }
        
        Ok(())
    }

    /// Leave a table and withdraw chips
    pub fn leave_table(ctx: Context<LeaveTable>) -> Result<()> {
        let table = &mut ctx.accounts.table;
        let player_state = &mut ctx.accounts.player_state;
        
        // Validate table state
        require!(
            table.status == TableStatus::Waiting || table.status == TableStatus::Finished,
            ErrorCode::CannotLeaveActiveGame
        );
        
        // Find player's index in the table
        let mut player_index = table.max_players as usize;
        for (i, player_pubkey) in table.players.iter().enumerate() {
            if *player_pubkey == ctx.accounts.player.key() {
                player_index = i;
                break;
            }
        }
        require!(player_index < table.max_players as usize, ErrorCode::PlayerNotAtTable);
        
        // Remove player from table
        table.players[player_index] = Pubkey::default();
        table.player_count = table.player_count.checked_sub(1).unwrap();
        
        // Transfer chips from table vault to player
        let seeds = &[
            b"table".as_ref(),
            table.table_id.as_bytes(),
            &[table.bump],
        ];
        let signer = &[&seeds[..]];
        
        let cpi_accounts = Transfer {
            from: ctx.accounts.table_vault.to_account_info(),
            to: ctx.accounts.player_token_account.to_account_info(),
            authority: ctx.accounts.table.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer);
        token::transfer(cpi_ctx, player_state.chips)?;
        
        // Mark player as inactive
        player_state.is_active = false;
        player_state.chips = 0;
        
        // If host is leaving and other players remain, transfer host status
        if ctx.accounts.player.key() == table.host && table.player_count > 0 {
            // Find first active player to be new host
            for player_pubkey in table.players.iter() {
                if *player_pubkey != Pubkey::default() && *player_pubkey != ctx.accounts.player.key() {
                    table.host = *player_pubkey;
                    break;
                }
            }
        }
        
        // If no players left, close the table
        if table.player_count == 0 {
            // In a real implementation, we would close the table account here
            // and return the rent to the host
        }
        
        Ok(())
    }
}

/// Helper function to advance to the next active player
fn advance_to_next_player(table: &mut Table) -> Result<()> {
    let start_index = table.current_player_index;
    loop {
        table.current_player_index = (table.current_player_index + 1) % table.player_count;
        
        // If we've gone all the way around, break
        if table.current_player_index == start_index {
            break;
        }
        
        // If we found an active player who hasn't folded, break
        let player_pubkey = table.players[table.current_player_index as usize];
        if player_pubkey != Pubkey::default() {
            // In a real implementation, we would check if the player is active and hasn't folded
            break;
        }
    }
    
    Ok(())
}

/// Helper function to check if the current betting round is complete
fn check_round_completion(ctx: Context<PlayerAction>) -> Result<()> {
    let table = &mut ctx.accounts.table;
    
    // Check if all active players have matched the highest bet or folded
    let mut round_complete = true;
    for (i, player_pubkey) in table.players.iter().enumerate() {
        if *player_pubkey == Pubkey::default() {
            continue;
        }
        
        let player_state = &ctx.accounts.player_states[i];
        if player_state.is_folded || !player_state.is_active || player_state.is_all_in {
            continue;
        }
        
        if player_state.current_bet < table.highest_bet {
            round_complete = false;
            break;
        }
    }
    
    if round_complete {
        // Reset bets for next round
        for player_state in ctx.accounts.player_states.iter_mut() {
            player_state.current_bet = 0;
        }
        
        table.highest_bet = 0;
        
        // Advance to next round
        match table.round {
            Round::PreFlop => {
                table.round = Round::Flop;
                // In a real implementation, we would reveal the flop cards here
            }
            Round::Flop => {
                table.round = Round::Turn;
                // In a real implementation, we would reveal the turn card here
            }
            Round::Turn => {
                table.round = Round::River;
                // In a real implementation, we would reveal the river card here
            }
            Round::River => {
                table.round = Round::Showdown;
                // In a real implementation, we would trigger showdown here
            }
            _ => {}
        }
        
        // Set current player to the one after the dealer
        table.current_player_index = (table.dealer_index + 1) % table.player_count;
    }
    
    Ok(())
}

/// Helper function to count active players who haven't folded
fn count_active_players(ctx: Context<PlayerAction>) -> usize {
    let table = &ctx.accounts.table;
    let mut count = 0;
    
    for (i, player_pubkey) in table.players.iter().enumerate() {
        if *player_pubkey == Pubkey::default() {
            continue;
        }
        
        let player_state = &ctx.accounts.player_states[i];
        if !player_state.is_folded && player_state.is_active {
            count += 1;
        }
    }
    
    count
}

/// Generate a shuffled deck of cards (simplified for this example)
fn generate_shuffled_deck(seed: u64) -> Vec<u8> {
    let mut deck: Vec<u8> = (0..52).collect();
    
    // Simple Fisher-Yates shuffle based on seed
    let mut rng = seed;
    for i in (1..52).rev() {
        // Simple LCG random number generator
        rng = (rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407)) % u64::MAX;
        let j = (rng % (i as u64 + 1)) as usize;
        deck.swap(i, j);
    }
    
    deck
}

/// Simplified poker hand evaluation (returns a numeric value representing hand strength)
fn evaluate_poker_hand(cards: &[u8]) -> u32 {
    // In a real implementation, this would be a proper poker hand evaluator
    // For simplicity, we're just returning a placeholder value
    42
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    
    #[account(
        init,
        payer = authority,
        space = 8 + GameAuthority::SIZE,
        seeds = [b"game_authority"],
        bump
    )]
    pub game_authority: Account<'info, GameAuthority>,
    
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct CreateTable<'info> {
    #[account(mut)]
    pub host: Signer<'info>,
    
    #[account(
        init,
        payer = host,
        space = 8 + Table::SIZE,
        seeds = [b"table", table_id.as_bytes()],
        bump
    )]
    pub table: Account<'info, Table>,
    
    #[account(
        init,
        payer = host,
        space = 8 + PlayerState::SIZE,
        seeds = [b"player_state", host.key().as_ref(), table.key().as_ref()],
        bump
    )]
    pub player_state: Account<'info, PlayerState>,
    
    #[account(mut)]
    pub host_token_account: Account<'info, TokenAccount>,
    
    #[account(
        init,
        payer = host,
        token::mint = mint,
        token::authority = table,
    )]
    pub table_vault: Account<'info, TokenAccount>,
    
    pub mint: Account<'info, token::Mint>,
    
    #[account(mut)]
    pub game_authority: Account<'info, GameAuthority>,
    
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct JoinTable<'info> {
    #[account(mut)]
    pub player: Signer<'info>,
    
    #[account(mut)]
    pub table: Account<'info, Table>,
    
    #[account(
        init,
        payer = player,
        space = 8 + PlayerState::SIZE,
        seeds = [b"player_state", player.key().as_ref(), table.key().as_ref()],
        bump
    )]
    pub player_state: Account<'info, PlayerState>,
    
    #[account(mut)]
    pub player_token_account: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub table_vault: Account<'info, TokenAccount>,
    
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct StartGame<'info> {
    #[account(mut)]
    pub host: Signer<'info>,
    
    #[account(mut, has_one = host)]
    pub table: Account<'info, Table>,
    
    /// CHECK: We're checking all player states in the instruction
    #[account(mut)]
    pub player_states: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct PlayerAction<'info> {
    #[account(mut)]
    pub player: Signer<'info>,
    
    #[account(mut)]
    pub table: Account<'info, Table>,
    
    #[account(
        mut,
        seeds = [b"player_state", player.key().as_ref(), table.key().as_ref()],
        bump = player_state.bump
    )]
    pub player_state: Account<'info, PlayerState>,
    
    /// CHECK: We're checking all player states in the instruction
    #[account(mut)]
    pub player_states: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct Showdown<'info> {
    #[account(mut)]
    pub host: Signer<'info>,
    
    #[account(mut, has_one = host)]
    pub table: Account<'info, Table>,
    
    /// CHECK: We're checking all player states in the instruction
    #[account(mut)]
    pub player_states: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct ResetTable<'info> {
    #[account(mut)]
    pub host: Signer<'info>,
    
    #[account(mut, has_one = host)]
    pub table: Account<'info, Table>,
    
    /// CHECK: We're checking all player states in the instruction
    #[account(mut)]
    pub player_states: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct LeaveTable<'info> {
    #[account(mut)]
    pub player: Signer<'info>,
    
    #[account(mut)]
    pub table: Account<'info, Table>,
    
    #[account(
        mut,
        seeds = [b"player_state", player.key().as_ref(), table.key().as_ref()],
        bump = player_state.bump
    )]
    pub player_state: Account<'info, PlayerState>,
    
    #[account(mut)]
    pub player_token_account: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub table_vault: Account<'info, TokenAccount>,
    
    pub token_program: Program<'info, Token>,
}

#[account]
pub struct GameAuthority {
    pub authority: Pubkey,
    pub fee_percentage: u8,
    pub total_games_played: u64,
    pub total_fees_collected: u64,
    pub bump: u8,
}

impl GameAuthority {
    pub const SIZE: usize = 32 + 1 + 8 + 8 + 1;
}

#[account]
pub struct Table {
    pub host: Pubkey,
    pub table_id: String,
    pub buy_in: u64,
    pub small_blind: u64,
    pub big_blind: u64,
    pub max_players: u8,
    pub is_private: bool,
    pub status: TableStatus,
    pub pot: u64,
    pub players: Vec<Pubkey>,
    pub player_count: u8,
    pub current_player_index: u8,
    pub dealer_index: u8,
    pub round: Round,
    pub highest_bet: u64,
    pub community_cards: [u8; 5],
    pub bump: u8,
}

impl Table {
    pub const SIZE: usize = 32 + 32 + 8 + 8 + 8 + 1 + 1 + 1 + 8 + (9 * 32) + 1 + 1 + 1 + 1 + 8 + (5 * 1) + 1;
}

#[account]
pub struct PlayerState {
    pub player: Pubkey,
    pub table: Pubkey,
    pub chips: u64,
    pub is_active: bool,
    pub is_folded: bool,
    pub is_all_in: bool,
    pub current_bet: u64,
    pub cards: [u8; 2],
    pub bump: u8,
}

impl PlayerState {
    pub const SIZE: usize = 32 + 32 + 8 + 1 + 1 + 1 + 8 + (2 * 1) + 1;
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq)]
pub enum TableStatus {
    Waiting,
    Playing,
    Finished,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq)]
pub enum Round {
    NotStarted,
    PreFlop,
    Flop,
    Turn,
    River,
    Showdown,
}

#[error_code]
pub enum ErrorCode {
    #[msg("Fee percentage too high")]
    FeeTooHigh,
    #[msg("Invalid player count")]
    InvalidPlayerCount,
    #[msg("Invalid blinds configuration")]
    InvalidBlinds,
    #[msg("Buy-in amount too small")]
    BuyInTooSmall,
    #[msg("Table ID too long")]
    TableIdTooLong,
    #[msg("Table is not in waiting state")]
    TableNotWaiting,
    #[msg("Table is full")]
    TableFull,
    #[msg("Not enough players to start the game")]
    NotEnoughPlayers,
    #[msg("Only the host can perform this action")]
    NotTableHost,
    #[msg("Game is not in progress")]
    GameNotInProgress,
    #[msg("Player has already folded")]
    PlayerFolded,
    #[msg("Player is not active")]
    PlayerNotActive,
    #[msg("It's not your turn")]
    NotPlayerTurn,
    #[msg("Insufficient chips")]
    InsufficientChips,
    #[msg("Bet amount too small")]
    BetTooSmall,
    #[msg("Cannot check when there are active bets")]
    CannotCheck,
    #[msg("Not in showdown round")]
    NotShowdownRound,
    #[msg("Game is not finished")]
    GameNotFinished,
    #[msg("Cannot leave an active game")]
    CannotLeaveActiveGame,
    #[msg("Player is not at this table")]
    PlayerNotAtTable,
}

