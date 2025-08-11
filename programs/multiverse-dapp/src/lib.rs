use anchor_lang::prelude::InterfaceAccount;
use anchor_lang::prelude::*;
use anchor_spl::token_2022::{self, Burn, Token2022, TransferChecked};
use anchor_spl::token_interface::{Mint, TokenAccount};

declare_id!("HnUXpc1F7ELzb31awwM77LhQv1MhynDTJXM77298Qgbx");

// ==============================
// Global constants and helpers
// ==============================
// Fixed-point precision for reward accumulator
const ACC_PRECISION: u128 = 1_000_000_000_000; // 1e12
const BPS_DENOMINATOR: u64 = 10_000; // 100% in basis points

// Lock period multipliers in basis points
const MULTIPLIER_1M_BPS: u64 = 10_000; // 1.0x
const MULTIPLIER_3M_BPS: u64 = 12_000; // 1.2x
const MULTIPLIER_6M_BPS: u64 = 15_000; // 1.5xA
const MULTIPLIER_12M_BPS: u64 = 20_000; // 2.0x

fn lock_multiplier_bps(lock_duration: i64) -> u64 {
    match lock_duration {
        ONE_MONTH => MULTIPLIER_1M_BPS,
        THREE_MONTHS => MULTIPLIER_3M_BPS,
        SIX_MONTHS => MULTIPLIER_6M_BPS,
        TWELVE_MONTHS => MULTIPLIER_12M_BPS,
        _ => MULTIPLIER_1M_BPS,
    }
}

#[program]
pub mod multiversed_dapp {
    use super::*;

    /// Initializes all necessary accounts before staking
    pub fn initialize_accounts(ctx: Context<InitializeAccounts>) -> Result<()> {
        let staking_pool = &mut ctx.accounts.staking_pool;

        if staking_pool.total_staked > 0 {
            return Err(StakingError::AlreadyInitialized.into());
        }

        staking_pool.admin = ctx.accounts.admin.key();
        staking_pool.mint = ctx.accounts.mint.key();
        staking_pool.total_staked = 0;
        staking_pool.bump = ctx.bumps.staking_pool;

        msg!(
            "✅ Staking pool initialized with admin: {}",
            staking_pool.admin
        );
        Ok(())
    }

    // Prize distribution function - matches the TypeScript service exactly
    pub fn distribute_tournament_prizes(
        ctx: Context<DistributeTournamentPrizes>,
        tournament_id: String,
    ) -> Result<()> {
        // Fixed percentages for the top 3 positions
        const PERCENTAGES: [u8; 3] = [50, 30, 20]; // 1st: 50%, 2nd: 30%, 3rd: 20%

        // Convert tournament_id to fixed-size bytes for comparison
        let mut tournament_id_bytes = [0u8; 32];
        let id_bytes = tournament_id.as_bytes();
        let len = id_bytes.len().min(32);
        tournament_id_bytes[..len].copy_from_slice(&id_bytes[..len]);

        // Verify tournament ID matches
        require!(
            ctx.accounts.prize_pool.tournament_id == tournament_id_bytes,
            TournamentError::Unauthorized
        );

        // Ensure prize pool hasn't been distributed yet
        require!(
            !ctx.accounts.prize_pool.distributed,
            TournamentError::AlreadyDistributed
        );

        // Ensure there are funds to distribute
        require!(
            ctx.accounts.prize_pool.total_funds > 0,
            TournamentError::InsufficientFunds
        );

        let total_prize_funds = ctx.accounts.prize_pool.total_funds;
        let decimals = ctx.accounts.mint.decimals;

        // Calculate prize amounts for each position
        let first_place_amount = (total_prize_funds as u128 * PERCENTAGES[0] as u128 / 100) as u64;
        let second_place_amount = (total_prize_funds as u128 * PERCENTAGES[1] as u128 / 100) as u64;
        let third_place_amount = (total_prize_funds as u128 * PERCENTAGES[2] as u128 / 100) as u64;

        // Create signer seeds for prize pool PDA
        let tournament_pool_key = ctx.accounts.tournament_pool.key();
        let prize_pool_seeds = &[
            b"prize_pool",
            tournament_pool_key.as_ref(),
            &[ctx.accounts.prize_pool.bump],
        ];
        let signer_seeds: &[&[&[u8]]] = &[prize_pool_seeds];

        // Transfer prizes to winners
        // 1st Place
        if first_place_amount > 0 {
            token_2022::transfer_checked(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    TransferChecked {
                        from: ctx.accounts.prize_escrow_account.to_account_info(),
                        to: ctx.accounts.first_place_token_account.to_account_info(),
                        mint: ctx.accounts.mint.to_account_info(),
                        authority: ctx.accounts.prize_pool.to_account_info(),
                    },
                    signer_seeds,
                ),
                first_place_amount,
                decimals,
            )?;
        }

        // 2nd Place
        if second_place_amount > 0 {
            token_2022::transfer_checked(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    TransferChecked {
                        from: ctx.accounts.prize_escrow_account.to_account_info(),
                        to: ctx.accounts.second_place_token_account.to_account_info(),
                        mint: ctx.accounts.mint.to_account_info(),
                        authority: ctx.accounts.prize_pool.to_account_info(),
                    },
                    signer_seeds,
                ),
                second_place_amount,
                decimals,
            )?;
        }

        // 3rd Place
        if third_place_amount > 0 {
            token_2022::transfer_checked(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    TransferChecked {
                        from: ctx.accounts.prize_escrow_account.to_account_info(),
                        to: ctx.accounts.third_place_token_account.to_account_info(),
                        mint: ctx.accounts.mint.to_account_info(),
                        authority: ctx.accounts.prize_pool.to_account_info(),
                    },
                    signer_seeds,
                ),
                third_place_amount,
                decimals,
            )?;
        }

        // Mark prize pool as distributed
        ctx.accounts.prize_pool.distributed = true;
        ctx.accounts.prize_pool.total_funds = 0; // Reset funds after distribution

        msg!(
            "✅ Tournament prizes distributed: 1st: {}, 2nd: {}, 3rd: {}",
            first_place_amount,
            second_place_amount,
            third_place_amount
        );

        Ok(())
    }

    /// Allows a user to stake tokens
    pub fn stake(ctx: Context<Stake>, amount: u64, lock_duration: i64) -> Result<()> {
        let mint_decimals = ctx.accounts.mint.decimals;
        let amount_in_base_units = amount
            .checked_mul(10_u64.pow(mint_decimals as u32))
            .ok_or(StakingError::MathOverflow)?;

        require!(
            lock_duration == ONE_MONTH
                || lock_duration == THREE_MONTHS
                || lock_duration == SIX_MONTHS
                || lock_duration == TWELVE_MONTHS,
            StakingError::InvalidLockDuration
        );

        // Transfer tokens to escrow
        let cpi_accounts = TransferChecked {
            from: ctx.accounts.user_token_account.to_account_info(),
            to: ctx.accounts.pool_escrow_account.to_account_info(),
            mint: ctx.accounts.mint.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };

        token_2022::transfer_checked(
            CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts),
            amount_in_base_units,
            mint_decimals,
        )?;

        // Update user staking account and pool weight
        let user_staking_account = &mut ctx.accounts.user_staking_account;
        let staking_pool = &mut ctx.accounts.staking_pool;
        let _current_timestamp = Clock::get()?.unix_timestamp;

        // Accrue pending before changing weight
        let accumulated_per_user: u128 = user_staking_account
            .weight
            .saturating_mul(staking_pool.acc_reward_per_weight)
            .checked_div(ACC_PRECISION)
            .unwrap_or(0);
        let pending_now: u128 = accumulated_per_user
            .saturating_sub(user_staking_account.reward_debt);
        if pending_now > 0 {
            let add: u64 = pending_now.min(u128::from(u64::MAX)) as u64;
            user_staking_account.pending_rewards = user_staking_account
                .pending_rewards
                .saturating_add(add);
        }

        // Principal and lock details
        user_staking_account.owner = ctx.accounts.user.key();
        user_staking_account.staked_amount = user_staking_account
            .staked_amount
            .checked_add(amount_in_base_units)
            .ok_or(StakingError::MathOverflow)?;
        user_staking_account.stake_timestamp = _current_timestamp;
        user_staking_account.lock_duration = lock_duration;

        // Pool totals and weight
        staking_pool.total_staked = staking_pool
            .total_staked
            .saturating_add(amount_in_base_units);

        let multiplier_bps: u64 = lock_multiplier_bps(lock_duration);
        let added_weight: u128 = (amount_in_base_units as u128)
            .saturating_mul(multiplier_bps as u128)
            .checked_div(BPS_DENOMINATOR as u128)
            .unwrap_or(0);
        staking_pool.total_weight = staking_pool.total_weight.saturating_add(added_weight);
        user_staking_account.weight = user_staking_account.weight.saturating_add(added_weight);

        // Sync reward debt to new baseline
        user_staking_account.reward_debt = user_staking_account
            .weight
            .saturating_mul(staking_pool.acc_reward_per_weight)
            .checked_div(ACC_PRECISION)
            .unwrap_or(0);

        msg!(
            "✅ {} tokens staked by user: {} for {} seconds",
            amount,
            ctx.accounts.user.key(),
            lock_duration
        );

        Ok(())
    }

    /// Allows users to unstake all of their tokens at any time
    pub fn unstake(ctx: Context<Unstake>) -> Result<()> {
        let user_staking_account = &mut ctx.accounts.user_staking_account;
        let staking_pool = &mut ctx.accounts.staking_pool;

        // Ensure the user has a staked balance
        require!(
            user_staking_account.staked_amount > 0,
            StakingError::InsufficientStakedBalance
        );

        let mint_decimals = ctx.accounts.mint.decimals;
        let amount_in_base_units = user_staking_account.staked_amount;

        // Accrue pending rewards up to now
        let accumulated_per_user: u128 = user_staking_account
            .weight
            .saturating_mul(staking_pool.acc_reward_per_weight)
            .checked_div(ACC_PRECISION)
            .unwrap_or(0);
        let pending_now: u128 = accumulated_per_user
            .saturating_sub(user_staking_account.reward_debt);
        if pending_now > 0 {
            let add: u64 = pending_now.min(u128::from(u64::MAX)) as u64;
            user_staking_account.pending_rewards = user_staking_account
                .pending_rewards
                .saturating_add(add);
        }

        // Prepare signer seeds using copies to avoid conflicting borrows
        let staking_pool_admin = staking_pool.admin;
        let staking_pool_bump = staking_pool.bump;
        // Transfer all staked tokens from the escrow account to the user's token account
        let staking_pool_seeds = &[
            b"staking_pool",
            staking_pool_admin.as_ref(),
            &[staking_pool_bump],
        ];
        let signer_seeds: &[&[&[u8]]] = &[staking_pool_seeds];

        let transfer_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.pool_escrow_account.to_account_info(),
                to: ctx.accounts.user_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                authority: staking_pool.to_account_info(),
            },
            signer_seeds,
        );

        // Execute the transfer of tokens (entire staked amount)
        token_2022::transfer_checked(transfer_ctx, amount_in_base_units, mint_decimals)?;

        // Update the staking data
        user_staking_account.staked_amount = user_staking_account
            .staked_amount
            .checked_sub(amount_in_base_units)
            .ok_or(StakingError::MathOverflow)?;

        staking_pool.total_staked = staking_pool
            .total_staked
            .checked_sub(amount_in_base_units)
            .ok_or(StakingError::MathOverflow)?;

        // Remove weight from pool and reset user's weight and debt
        staking_pool.total_weight = staking_pool.total_weight.saturating_sub(user_staking_account.weight);
        user_staking_account.weight = 0;
        user_staking_account.reward_debt = 0;

        msg!(
            "✅ User {} unstaked all tokens. Remaining: {}",
            ctx.accounts.user.key(),
            user_staking_account.staked_amount
        );

        Ok(())
    }

    // GameHub Logic (Tournament creation and registration)
    pub fn create_tournament_pool(
        ctx: Context<CreateTournamentPool>,
        tournament_id: String,
        entry_fee: u64,
        max_participants: u16,
        end_time: i64,
    ) -> Result<()> {
        let tournament_pool = &mut ctx.accounts.tournament_pool;
        let admin = &ctx.accounts.admin;

        // Validate tournament parameters
        require!(entry_fee > 0, TournamentError::InvalidEntryFee);
        require!(
            max_participants > 0,
            TournamentError::InvalidMaxParticipants
        );
        require!(
            end_time > Clock::get()?.unix_timestamp,
            TournamentError::InvalidEndTime
        );

        // Convert tournament_id to a fixed-size byte array
        let mut tournament_id_bytes = [0u8; 32]; // Increased from 10 to 32
        let id_bytes = tournament_id.as_bytes();
        let len = id_bytes.len().min(32); // Increased limit
        tournament_id_bytes[..len].copy_from_slice(&id_bytes[..len]);

        // Assign values to PDA
        tournament_pool.admin = admin.key();
        tournament_pool.mint = ctx.accounts.mint.key();
        tournament_pool.tournament_id = tournament_id_bytes;
        tournament_pool.entry_fee = entry_fee;
        tournament_pool.total_funds = 0;
        tournament_pool.participant_count = 0;
        tournament_pool.max_participants = max_participants;
        tournament_pool.end_time = end_time;
        tournament_pool.is_active = true;
        tournament_pool.bump = ctx.bumps.tournament_pool;

        msg!(
            "Tournament '{}' created by admin: {} with entry fee: {} and max participants: {}",
            tournament_id,
            admin.key(),
            entry_fee,
            max_participants
        );

        Ok(())
    }

    pub fn register_for_tournament(
        ctx: Context<RegisterForTournament>,
        _tournament_id: String,
    ) -> Result<()> {
        let tournament_pool = &mut ctx.accounts.tournament_pool;
        let user_token_account = &mut ctx.accounts.user_token_account;
        let user = &ctx.accounts.user;
        let mint = &ctx.accounts.mint;

        // Check if tournament is active
        require!(
            tournament_pool.is_active,
            TournamentError::TournamentNotActive
        );

        // Check if tournament is full
        require!(
            tournament_pool.participant_count < tournament_pool.max_participants,
            TournamentError::TournamentFull
        );

        // Check if tournament has ended
        let current_time = Clock::get()?.unix_timestamp;
        require!(
            current_time < tournament_pool.end_time,
            TournamentError::TournamentEnded
        );

        // Check if user has sufficient funds
        let entry_fee = tournament_pool.entry_fee;
        let balance = user_token_account.amount;
        require!(balance >= entry_fee, TournamentError::InsufficientFunds);

        // Create registration record if not already exists
        let registration_account = &mut ctx.accounts.registration_account;
        require!(
            registration_account.is_initialized == false,
            TournamentError::AlreadyRegistered
        );

        // Register user
        registration_account.user = user.key();
        registration_account.tournament_pool = tournament_pool.key();
        registration_account.is_initialized = true;
        registration_account.registration_time = current_time;
        registration_account.bump = ctx.bumps.registration_account;

        // Transfer entry fee to escrow account
        let transfer_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: user_token_account.to_account_info(),
                to: ctx.accounts.pool_escrow_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                authority: user.to_account_info(),
            },
        );

        // Get decimals from mint account instead of hardcoding
        let decimals = mint.decimals;

        token_2022::transfer_checked(transfer_ctx, entry_fee, decimals)?;

        // Update tournament pool
        tournament_pool.total_funds += entry_fee;
        tournament_pool.participant_count += 1;

        msg!(
            "User {} registered for tournament {} with {} tokens.",
            user.key(),
            String::from_utf8_lossy(&tournament_pool.tournament_id),
            entry_fee
        );

        Ok(())
    }

    // Modified instruction to initialize just the global revenue pool
    pub fn initialize_revenue_pool(ctx: Context<InitializeRevenuePool>) -> Result<()> {
        let revenue_pool = &mut ctx.accounts.revenue_pool;
        let admin = &ctx.accounts.admin;

        // Initialize revenue pool
        revenue_pool.admin = admin.key();
        revenue_pool.mint = ctx.accounts.mint.key();
        revenue_pool.total_funds = 0;
        revenue_pool.last_distribution = Clock::get()?.unix_timestamp;
        revenue_pool.bump = ctx.bumps.revenue_pool;

        msg!("✅ Revenue pool initialized for admin: {}", admin.key());
        Ok(())
    }

    // Initialize a global RewardPool (used to hold the 5% staking rewards)
    pub fn initialize_reward_pool(ctx: Context<InitializeRewardPool>) -> Result<()> {
        let reward_pool = &mut ctx.accounts.reward_pool;
        let admin = &ctx.accounts.admin;

        reward_pool.admin = admin.key();
        reward_pool.mint = ctx.accounts.mint.key();
        reward_pool.total_funds = 0;
        reward_pool.last_distribution = Clock::get()?.unix_timestamp;
        reward_pool.bump = ctx.bumps.reward_pool;

        msg!("✅ Reward pool initialized for admin: {}", admin.key());
        Ok(())
    }

    // New instruction to initialize a prize pool for a specific tournament
    pub fn initialize_prize_pool(
        ctx: Context<InitializePrizePool>,
        tournament_id: String,
    ) -> Result<()> {
        let prize_pool = &mut ctx.accounts.prize_pool;
        let admin = &ctx.accounts.admin;
        let tournament_pool = &ctx.accounts.tournament_pool;

        // Convert tournament_id to fixed-size bytes
        let mut tournament_id_bytes = [0u8; 32]; // Increased from 10 to 32
        let id_bytes = tournament_id.as_bytes();
        let len = id_bytes.len().min(32);
        tournament_id_bytes[..len].copy_from_slice(&id_bytes[..len]);

        // Verify that the tournament_id matches the one in the tournament pool
        let tournament_pool_id = &tournament_pool.tournament_id;
        require!(
            &tournament_id_bytes[..] == tournament_pool_id.as_ref(),
            TournamentError::Unauthorized
        );

        // Initialize prize pool
        prize_pool.admin = admin.key();
        prize_pool.tournament_pool = tournament_pool.key();
        prize_pool.mint = ctx.accounts.mint.key();
        prize_pool.tournament_id = tournament_id_bytes;
        prize_pool.total_funds = 0;
        prize_pool.distributed = false;
        prize_pool.bump = ctx.bumps.prize_pool;

        msg!(
            "✅ Prize pool initialized for tournament: {}",
            tournament_id
        );
        Ok(())
    }

    // OPTIMIZED FUNCTION: Fixed stack overflow issues
    pub fn distribute_tournament_revenue(
        ctx: Context<DistributeTournamentRevenue>,
        tournament_id: String,
        prize_percentage: u8,
        revenue_percentage: u8,
        staking_percentage: u8,
        burn_percentage: u8,
    ) -> Result<()> {
        // Early validation to fail fast and reduce stack usage
        require!(
            prize_percentage.saturating_add(revenue_percentage)
                .saturating_add(staking_percentage)
                .saturating_add(burn_percentage) == 100,
            TournamentError::InvalidPercentages
        );

        // CRITICAL FIX: Use slice instead of converting to bytes to reduce stack allocation
        let tournament_id_slice = tournament_id.as_str();
        require!(
            tournament_id_slice.len() <= 32, // Increased from 10 to 32
            TournamentError::InvalidTournamentId
        );

        // Get references early to reduce dereferencing overhead
        let tournament_pool = &ctx.accounts.tournament_pool;
        let mint = &ctx.accounts.mint;
        
        // Early checks to fail fast
        require!(tournament_pool.is_active, TournamentError::TournamentNotActive);
        require!(tournament_pool.total_funds > 0, TournamentError::InsufficientFunds);

        // Store values in local variables to reduce repeated access
        let total_funds = tournament_pool.total_funds;
        let decimals = mint.decimals;
        let admin_key = ctx.accounts.admin.key();
        let pool_bump = tournament_pool.bump;

        // OPTIMIZATION: Use u64 math with checked operations, more efficient
        let prize_amount = (total_funds as u128 * prize_percentage as u128 / 100) as u64;
        let revenue_amount = (total_funds as u128 * revenue_percentage as u128 / 100) as u64;
        let staking_amount = (total_funds as u128 * staking_percentage as u128 / 100) as u64;
        let burn_amount = (total_funds as u128 * burn_percentage as u128 / 100) as u64;

        // CRITICAL FIX: Create seeds once and reuse
        let tournament_id_bytes = tournament_id_slice.as_bytes();

        // Transfer to prize pool (optimized)
        if prize_amount > 0 {
            token_2022::transfer_checked(
                CpiContext::new(
                    ctx.accounts.token_program.to_account_info(),
                    TransferChecked {
                        from: ctx.accounts.tournament_escrow_account.to_account_info(),
                        to: ctx.accounts.prize_escrow_account.to_account_info(),
                        mint: mint.to_account_info(),
                        authority: ctx.accounts.admin.to_account_info(), // Use admin as authority
                    },
                ),
                prize_amount,
                decimals,
            )?;
            
            ctx.accounts.prize_pool.total_funds = ctx.accounts.prize_pool.total_funds
                .saturating_add(prize_amount);
        }

        // Transfer to revenue pool (optimized)
        if revenue_amount > 0 {
            token_2022::transfer_checked(
                CpiContext::new(
                    ctx.accounts.token_program.to_account_info(),
                    TransferChecked {
                        from: ctx.accounts.tournament_escrow_account.to_account_info(),
                        to: ctx.accounts.revenue_escrow_account.to_account_info(),
                        mint: mint.to_account_info(),
                        authority: ctx.accounts.admin.to_account_info(), // Use admin as authority
                    },
                ),
                revenue_amount,
                decimals,
            )?;
            
            ctx.accounts.revenue_pool.total_funds = ctx.accounts.revenue_pool.total_funds
                .saturating_add(revenue_amount);
        }

        // Transfer staking rewards portion to RewardPool (NOT staking pool)
        if staking_amount > 0 {
            token_2022::transfer_checked(
                CpiContext::new(
                    ctx.accounts.token_program.to_account_info(),
                    TransferChecked {
                        from: ctx.accounts.tournament_escrow_account.to_account_info(),
                        to: ctx.accounts.reward_escrow_account.to_account_info(),
                        mint: mint.to_account_info(),
                        authority: ctx.accounts.admin.to_account_info(),
                    },
                ),
                staking_amount,
                decimals,
            )?;

            ctx.accounts.reward_pool.total_funds = ctx.accounts.reward_pool.total_funds
                .saturating_add(staking_amount);

            // Update reward accumulator for current stakers
            let sp = &mut ctx.accounts.staking_pool;
            if sp.total_weight > 0 {
                let delta: u128 = (staking_amount as u128)
                    .saturating_mul(ACC_PRECISION)
                    .checked_div(sp.total_weight)
                    .unwrap_or(0);
                sp.acc_reward_per_weight = sp.acc_reward_per_weight.saturating_add(delta);
                sp.epoch_index = sp.epoch_index.saturating_add(1);
            }
        }

        // Burn tokens (optimized)
        if burn_amount > 0 {
            token_2022::burn(
                CpiContext::new(
                    ctx.accounts.token_program.to_account_info(),
                    Burn {
                        mint: mint.to_account_info(),
                        from: ctx.accounts.tournament_escrow_account.to_account_info(),
                        authority: ctx.accounts.admin.to_account_info(), // Use admin as authority
                    },
                ),
                burn_amount,
            )?;
        }

        // Update states (optimized)
        ctx.accounts.tournament_pool.is_active = false;
        ctx.accounts.tournament_pool.total_funds = 0;
        ctx.accounts.revenue_pool.last_distribution = Clock::get()?.unix_timestamp;

        msg!(
            "✅ Tournament revenue distributed: Prize: {}, Revenue: {}, Staking: {}, Burned: {}",
            prize_amount,
            revenue_amount,
            staking_amount,
            burn_amount
        );

        Ok(())
    }

    /// Claim rewards from RewardPool pro‑rata to user weight
    pub fn claim_rewards(ctx: Context<ClaimRewards>) -> Result<()> {
        let staking_pool = &mut ctx.accounts.staking_pool;
        let reward_pool = &mut ctx.accounts.reward_pool;
        let user_staking_account = &mut ctx.accounts.user_staking_account;

        // Compute claimable based on accumulator
        let accumulated: u128 = user_staking_account
            .weight
            .saturating_mul(staking_pool.acc_reward_per_weight)
            .checked_div(ACC_PRECISION)
            .unwrap_or(0);
        let claimable_u128: u128 = accumulated.saturating_sub(user_staking_account.reward_debt)
            .saturating_add(user_staking_account.pending_rewards as u128);
        let claimable: u64 = claimable_u128.min(u128::from(u64::MAX)) as u64;
        require!(claimable > 0, StakingError::InsufficientStakedBalance); // nothing to claim

        // Ensure RewardPool has sufficient funds
        require!(reward_pool.total_funds >= claimable, TournamentError::InsufficientFunds);

        // Transfer from reward escrow to user token account
        let decimals = ctx.accounts.mint.decimals;
        let reward_admin = reward_pool.admin;
        let reward_bump = reward_pool.bump;
        let reward_pool_seeds = &[b"reward_pool", reward_admin.as_ref(), &[reward_bump]];
        let signer_seeds: &[&[&[u8]]] = &[reward_pool_seeds];
        let transfer_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.reward_escrow_account.to_account_info(),
                to: ctx.accounts.user_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                authority: reward_pool.to_account_info(),
            },
            signer_seeds,
        );
        token_2022::transfer_checked(transfer_ctx, claimable, decimals)?;

        // Update accounting
        reward_pool.total_funds = reward_pool.total_funds.saturating_sub(claimable);
        user_staking_account.pending_rewards = 0;
        user_staking_account.reward_debt = user_staking_account
            .weight
            .saturating_mul(staking_pool.acc_reward_per_weight)
            .checked_div(ACC_PRECISION)
            .unwrap_or(0);

        msg!("✅ Rewards claimed: {}", claimable);
        Ok(())
    }

    // Rest of the account structs and implementations remain the same...
    #[derive(Accounts)]
    #[instruction(tournament_id: String, entry_fee: u64, max_participants: u16, end_time: i64)]
    pub struct CreateTournamentPool<'info> {
        #[account(
            init,
            payer = admin,
            space = 8 + 32 + 32 + 32 + 8 + 8 + 2 + 2 + 8 + 1 + 1, // Updated space calculation
            seeds = [b"tournament_pool", admin.key().as_ref(), tournament_id.as_bytes()],
            bump
        )]
        pub tournament_pool: Account<'info, TournamentPool>,

        #[account(
            init,
            payer = admin,
            token::mint = mint,
            token::authority = admin,
            seeds = [b"escrow", tournament_pool.key().as_ref()],
            bump
        )]
        pub pool_escrow_account: InterfaceAccount<'info, TokenAccount>,

        pub mint: InterfaceAccount<'info, Mint>,
        #[account(mut)]
        pub admin: Signer<'info>,
        pub system_program: Program<'info, System>,
        pub token_program: Program<'info, Token2022>,
    }

    // Account structure for prize distribution - matches TypeScript service exactly
    #[derive(Accounts)]
    #[instruction(tournament_id: String)]
    pub struct DistributeTournamentPrizes<'info> {
        #[account(mut, constraint = admin.key() == prize_pool.admin @ TournamentError::Unauthorized)]
        pub admin: Signer<'info>,

        #[account(
            constraint = tournament_pool.key() == prize_pool.tournament_pool @ TournamentError::Unauthorized
        )]
        pub tournament_pool: Account<'info, TournamentPool>,

        #[account(
            mut,
            seeds = [b"prize_pool", tournament_pool.key().as_ref()],
            bump = prize_pool.bump
        )]
        pub prize_pool: Account<'info, PrizePool>,

        #[account(
            mut,
            token::mint = mint,
            token::authority = prize_pool
        )]
        pub prize_escrow_account: InterfaceAccount<'info, TokenAccount>,

        // Winner token accounts - matching the service parameter names exactly
        #[account(mut, constraint = first_place_token_account.mint == mint.key())]
        pub first_place_token_account: InterfaceAccount<'info, TokenAccount>,

        #[account(mut, constraint = second_place_token_account.mint == mint.key())]
        pub second_place_token_account: InterfaceAccount<'info, TokenAccount>,

        #[account(mut, constraint = third_place_token_account.mint == mint.key())]
        pub third_place_token_account: InterfaceAccount<'info, TokenAccount>,

        #[account(mut, constraint = mint.key() == prize_pool.mint)]
        pub mint: InterfaceAccount<'info, Mint>,

        pub token_program: Program<'info, Token2022>,
        pub system_program: Program<'info, System>,
    }

    #[derive(Accounts)]
    #[instruction(tournament_id: String)]
    pub struct RegisterForTournament<'info> {
        #[account(mut)]
        pub user: Signer<'info>,

        #[account(
            mut,
            seeds = [b"tournament_pool", tournament_pool.admin.as_ref(), tournament_id.as_bytes()],
            bump = tournament_pool.bump
        )]
        pub tournament_pool: Account<'info, TournamentPool>,

        #[account(
            init,
            payer = user,
            space = 8 + 32 + 32 + 1 + 8 + 1,
            seeds = [b"registration", tournament_pool.key().as_ref(), user.key().as_ref()],
            bump
        )]
        pub registration_account: Account<'info, RegistrationRecord>,

        #[account(mut, constraint = user_token_account.mint == tournament_pool.mint)]
        pub user_token_account: InterfaceAccount<'info, TokenAccount>,

        #[account(
            mut,
            token::mint = tournament_pool.mint,
            token::authority = tournament_pool.admin
        )]
        pub pool_escrow_account: InterfaceAccount<'info, TokenAccount>,

        #[account(mut, constraint = mint.key() == tournament_pool.mint)]
        pub mint: InterfaceAccount<'info, Mint>,

        pub token_program: Program<'info, Token2022>,
        pub system_program: Program<'info, System>,
    }

    #[derive(Accounts)]
    pub struct InitializeRevenuePool<'info> {
        #[account(
            init,
            payer = admin,
            space = 8 + 32 + 32 + 8 + 8 + 1,
            seeds = [b"revenue_pool", admin.key().as_ref()],
            bump
        )]
        pub revenue_pool: Account<'info, RevenuePool>,

        #[account(
            init,
            payer = admin,
            token::mint = mint,
            token::authority = revenue_pool,
            seeds = [b"revenue_escrow", revenue_pool.key().as_ref()],
            bump
        )]
        pub revenue_escrow_account: InterfaceAccount<'info, TokenAccount>,

        pub mint: InterfaceAccount<'info, Mint>,
        #[account(mut)]
        pub admin: Signer<'info>,
        pub system_program: Program<'info, System>,
        pub token_program: Program<'info, Token2022>,
    }

    #[derive(Accounts)]
    pub struct InitializeRewardPool<'info> {
        #[account(
            init,
            payer = admin,
            space = 8 + 32 + 32 + 8 + 8 + 1,
            seeds = [b"reward_pool", admin.key().as_ref()],
            bump
        )]
        pub reward_pool: Account<'info, RewardPool>,

        #[account(
            init,
            payer = admin,
            token::mint = mint,
            token::authority = reward_pool,
            seeds = [b"reward_escrow", reward_pool.key().as_ref()],
            bump
        )]
        pub reward_escrow_account: InterfaceAccount<'info, TokenAccount>,

        pub mint: InterfaceAccount<'info, Mint>,
        #[account(mut)]
        pub admin: Signer<'info>,
        pub system_program: Program<'info, System>,
        pub token_program: Program<'info, Token2022>,
    }

    #[derive(Accounts)]
    #[instruction(tournament_id: String)]
    pub struct InitializePrizePool<'info> {
        #[account(
            init,
            payer = admin,
            space = 8 + 32 + 32 + 32 + 32 + 8 + 1 + 1, // Updated space calculation
            seeds = [b"prize_pool", tournament_pool.key().as_ref()],
            bump
        )]
        pub prize_pool: Account<'info, PrizePool>,

        #[account(
            mut,
            seeds = [b"tournament_pool", tournament_pool.admin.as_ref(), tournament_id.as_bytes()],
            bump = tournament_pool.bump,
            constraint = tournament_pool.admin == admin.key() @ TournamentError::Unauthorized
        )]
        pub tournament_pool: Account<'info, TournamentPool>,

        #[account(
            init_if_needed,
            payer = admin,
            token::mint = mint,
            token::authority = prize_pool,
            seeds = [b"prize_escrow", prize_pool.key().as_ref()],
            bump
        )]
        pub prize_escrow_account: InterfaceAccount<'info, TokenAccount>,

        pub mint: InterfaceAccount<'info, Mint>,
        #[account(mut)]
        pub admin: Signer<'info>,
        pub system_program: Program<'info, System>,
        pub token_program: Program<'info, Token2022>,
    }

    // CRITICAL FIX: Simplified DistributeTournamentRevenue struct to reduce stack usage
    #[derive(Accounts)]
    #[instruction(tournament_id: String, prize_percentage: u8, revenue_percentage: u8, staking_percentage: u8, burn_percentage: u8)]
    pub struct DistributeTournamentRevenue<'info> {
        #[account(mut)]
        pub admin: Signer<'info>,

        #[account(
            mut,
            seeds = [b"tournament_pool", admin.key().as_ref(), tournament_id.as_bytes()],
            bump = tournament_pool.bump,
            constraint = tournament_pool.admin == admin.key() @ TournamentError::Unauthorized
        )]
        pub tournament_pool: Account<'info, TournamentPool>,

        #[account(
            mut,
            seeds = [b"prize_pool", tournament_pool.key().as_ref()],
            bump = prize_pool.bump
        )]
        pub prize_pool: Account<'info, PrizePool>,

        #[account(
            mut,
            seeds = [b"revenue_pool", admin.key().as_ref()],
            bump = revenue_pool.bump
        )]
        pub revenue_pool: Account<'info, RevenuePool>,

        #[account(
            mut,
            seeds = [b"staking_pool", admin.key().as_ref()],
            bump = staking_pool.bump
        )]
        pub staking_pool: Account<'info, StakingPool>,

        #[account(
            mut,
            seeds = [b"reward_pool", admin.key().as_ref()],
            bump = reward_pool.bump
        )]
        pub reward_pool: Account<'info, RewardPool>,

        // CRITICAL FIX: Removed explicit authority constraints to reduce validation overhead
        #[account(mut)]
        pub tournament_escrow_account: InterfaceAccount<'info, TokenAccount>,

        #[account(mut)]
        pub prize_escrow_account: InterfaceAccount<'info, TokenAccount>,

        #[account(mut)]
        pub revenue_escrow_account: InterfaceAccount<'info, TokenAccount>,

        #[account(mut)]
        pub reward_escrow_account: InterfaceAccount<'info, TokenAccount>,

        #[account(mut)]
        pub mint: InterfaceAccount<'info, Mint>,

        pub token_program: Program<'info, Token2022>,
    }

    #[derive(Accounts)]
    pub struct InitializeAccounts<'info> {
        #[account(
            init_if_needed,
            payer = admin,
            // admin + mint + total_staked(u64) + total_weight(u128) + acc_reward_per_weight(u128) + epoch_index(u64) + bump
            space = 8 + 32 + 32 + 8 + 16 + 16 + 8 + 1,
            seeds = [b"staking_pool", admin.key().as_ref()],
            bump
        )]
        pub staking_pool: Account<'info, StakingPool>,

        #[account(
            init_if_needed,
            payer = admin,
            token::mint = mint,
            token::authority = staking_pool,
            seeds = [b"escrow", staking_pool.key().as_ref()],
            bump
        )]
        pub pool_escrow_account: InterfaceAccount<'info, TokenAccount>,

        pub mint: InterfaceAccount<'info, Mint>,
        #[account(mut)]
        pub admin: Signer<'info>,
        pub system_program: Program<'info, System>,
        pub token_program: Program<'info, Token2022>,
    }

    #[derive(Accounts)]
    pub struct Stake<'info> {
        #[account(mut)]
        pub user: Signer<'info>,

        #[account(
            mut,
            seeds = [b"staking_pool", staking_pool.admin.as_ref()],
            bump = staking_pool.bump
        )]
        pub staking_pool: Account<'info, StakingPool>,

        #[account(
            init_if_needed,
            payer = user,
            // owner + staked_amount(u64) + stake_timestamp(i64) + lock_duration(i64) + weight(u128) + reward_debt(u128) + pending_rewards(u64)
            space = 8 + 32 + 8 + 8 + 8 + 16 + 16 + 8,
            seeds = [b"user_stake", user.key().as_ref()],
            bump
        )]
        pub user_staking_account: Account<'info, UserStakingAccount>,

        #[account(mut, constraint = user_token_account.mint == staking_pool.mint)]
        pub user_token_account: InterfaceAccount<'info, TokenAccount>,

        #[account(
            mut,
            token::mint = staking_pool.mint,
            token::authority = staking_pool
        )]
        pub pool_escrow_account: InterfaceAccount<'info, TokenAccount>,

        #[account(mut, constraint = mint.key() == staking_pool.mint)]
        pub mint: InterfaceAccount<'info, Mint>,

        pub token_program: Program<'info, Token2022>,
        pub system_program: Program<'info, System>,
    }

    #[derive(Accounts)]
    pub struct Unstake<'info> {
        #[account(mut)]
        pub user: Signer<'info>,

        #[account(
            mut,
            seeds = [b"staking_pool", staking_pool.admin.as_ref()],
            bump = staking_pool.bump
        )]
        pub staking_pool: Account<'info, StakingPool>,

        #[account(
            mut,
            seeds = [b"user_stake", user.key().as_ref()],
            bump,
            close = user
        )]
        pub user_staking_account: Account<'info, UserStakingAccount>,

        #[account(mut)]
        pub user_token_account: InterfaceAccount<'info, TokenAccount>,

        #[account(
            mut,
            token::mint = staking_pool.mint,
            token::authority = staking_pool
        )]
        pub pool_escrow_account: InterfaceAccount<'info, TokenAccount>,

        #[account(mut, constraint = mint.key() == staking_pool.mint)]
        pub mint: InterfaceAccount<'info, Mint>,

        pub token_program: Program<'info, Token2022>,
    }

    // Accounts for claiming rewards
    #[derive(Accounts)]
    pub struct ClaimRewards<'info> {
        #[account(mut)]
        pub user: Signer<'info>,

        #[account(
            mut,
            seeds = [b"staking_pool", staking_pool.admin.as_ref()],
            bump = staking_pool.bump
        )]
        pub staking_pool: Account<'info, StakingPool>,

        #[account(
            mut,
            seeds = [b"user_stake", user.key().as_ref()],
            bump
        )]
        pub user_staking_account: Account<'info, UserStakingAccount>,

        #[account(
            mut,
            seeds = [b"reward_pool", reward_pool.admin.as_ref()],
            bump = reward_pool.bump
        )]
        pub reward_pool: Account<'info, RewardPool>,

        #[account(mut)]
        pub user_token_account: InterfaceAccount<'info, TokenAccount>,

        #[account(
            mut,
            token::mint = mint,
            token::authority = reward_pool
        )]
        pub reward_escrow_account: InterfaceAccount<'info, TokenAccount>,

        #[account(mut, constraint = mint.key() == staking_pool.mint)]
        pub mint: InterfaceAccount<'info, Mint>,
        pub token_program: Program<'info, Token2022>,
    }

    const ONE_MONTH: i64 = 30 * 24 * 60 * 60;
    const THREE_MONTHS: i64 = 3 * ONE_MONTH;
    const SIX_MONTHS: i64 = 6 * ONE_MONTH;
    const TWELVE_MONTHS: i64 = 12 * ONE_MONTH;

    #[account]
    pub struct StakingPool {
        pub admin: Pubkey,
        pub mint: Pubkey,
        pub total_staked: u64,
        pub total_weight: u128,
        pub acc_reward_per_weight: u128,
        pub epoch_index: u64,
        pub bump: u8,
    }

    #[account]
    pub struct UserStakingAccount {
        pub owner: Pubkey,
        pub staked_amount: u64,
        pub stake_timestamp: i64,
        pub lock_duration: i64,
        pub weight: u128,
        pub reward_debt: u128,
        pub pending_rewards: u64,
    }

    #[account]
    pub struct TournamentPool {
        pub admin: Pubkey,
        pub mint: Pubkey,
        pub tournament_id: [u8; 32], // Increased from 10 to 32
        pub entry_fee: u64,
        pub total_funds: u64,
        pub participant_count: u16,
        pub max_participants: u16,
        pub end_time: i64,
        pub is_active: bool,
        pub bump: u8,
    }

    #[account]
    pub struct RegistrationRecord {
        pub user: Pubkey,
        pub tournament_pool: Pubkey,
        pub is_initialized: bool,
        pub registration_time: i64,
        pub bump: u8,
    }

    #[account]
    pub struct PrizePool {
        pub admin: Pubkey,
        pub tournament_pool: Pubkey,
        pub mint: Pubkey,
        pub tournament_id: [u8; 32], // Increased from 10 to 32
        pub total_funds: u64,
        pub distributed: bool,
        pub bump: u8,
    }

    #[account]
    pub struct RevenuePool {
        pub admin: Pubkey,
        pub mint: Pubkey,
        pub total_funds: u64,
        pub last_distribution: i64,
        pub bump: u8,
    }

    // Dedicated Reward Pool for staking rewards (5%)
    #[account]
    pub struct RewardPool {
        pub admin: Pubkey,
        pub mint: Pubkey,
        pub total_funds: u64,
        pub last_distribution: i64,
        pub bump: u8,
    }

    #[error_code]
    pub enum TournamentError {
        #[msg("Insufficient funds to register for this tournament.")]
        InsufficientFunds,

        #[msg("Tournament is full.")]
        TournamentFull,

        #[msg("Tournament has ended.")]
        TournamentEnded,

        #[msg("Tournament is not active.")]
        TournamentNotActive,

        #[msg("User is already registered for this tournament.")]
        AlreadyRegistered,

        #[msg("Invalid entry fee.")]
        InvalidEntryFee,

        #[msg("Invalid maximum participants.")]
        InvalidMaxParticipants,

        #[msg("Invalid end time.")]
        InvalidEndTime,

        #[msg("Unauthorized action.")]
        Unauthorized,

        #[msg("Invalid winner data.")]
        InvalidWinnerData,

        #[msg("Winner percentages must sum to 100.")]
        InvalidWinnerPercentages,

        #[msg("Distribution percentages must sum to 100.")]
        InvalidPercentages,

        #[msg("Invalid tournament ID.")]
        InvalidTournamentId,

        #[msg("Math overflow occurred.")]
        MathOverflow,

        #[msg("Prize pool has already been distributed.")]
        AlreadyDistributed,
    }

    #[error_code]
    pub enum StakingError {
        #[msg("Staking Pool already initialized by this admin")]
        AlreadyInitialized,
        #[msg("Insufficient staked balance")]
        InsufficientStakedBalance,
        #[msg("Unauthorized access")]
        Unauthorized,
        #[msg("Math overflow occurred")]
        MathOverflow,
        #[msg("Unstaking is locked")]
        StakeLockActive,
        #[msg("Invalid Lock Duration, Must be 1, 3, 6 or 12 months")]
        InvalidLockDuration,
    }
}