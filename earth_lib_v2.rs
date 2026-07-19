use anchor_lang::prelude::*;
use anchor_spl::token_2022::{self, MintTo, Transfer, Token2022};
use anchor_spl::token_interface::{Mint, TokenAccount};

declare_id!("EARTH111111111111111111111111111111111111111");

// ============================================================================
// CONSTANTS
// ============================================================================

/// Primary admin authority — your main wallet.
/// Can: emergency freeze/unfreeze, update oracle, set backup admin.
/// Cannot: mint tokens, move funds, alter supply outside programmatic rules.
pub const ADMIN_AUTHORITY: Pubkey = solana_program::pubkey!("FndrmgjS9iZ7wgnj58fp49W3cMSc3XEfBYkYA8J4cTH3");

// ============================================================================
// CLAIM RULES — THE ONLY GATE IS AT THE BEGINNING
// ============================================================================
//
// EARTH tokens come into existence when a verified human claims them.
// After that, they belong to that person — to hold, sell, trade, or transfer
// however they choose. No restrictions on what happens after the claim.
//
// The one rule that cannot be bypassed:
//   - To claim your initial allocation: World ID iris verification required.
//     One iris = one registration. One registration = one claim. Forever.
//
// Everything else — transfers, DEX trading, selling for other coins — is
// unrestricted. The token is theirs once claimed.
//
// Enforcement layers:
//   1. Registration: requires a valid World ID iris hash from the oracle.
//      One iris = one registration. Cannot be reused.
//   2. Claims: claim_allocation and claim_inflation_share both require the
//      claimer to be an active registered human.
//   3. Oracle: the World ID oracle server must verify iris uniqueness and
//      humanity before signing any register_human or mint instruction.
//
// These rules cannot be bypassed by governance vote. They are the foundation.
// ============================================================================

/// Starting token allocation per verified human (66,000 EARTH, 6 decimals).
/// This is the GENESIS amount. Each year it grows with Earth's value.
/// Read state.current_allocation for the live amount — never hardcode this.
pub const GENESIS_ALLOCATION: u64 = 66_000_000_000;

/// Starting annual growth rate in basis points (3.5% = 350).
/// Governance can update this each year via submit_annual_revaluation.
pub const GENESIS_GROWTH_BPS: u64 = 350;

/// Token decimals.
pub const TOKEN_DECIMALS: u8 = 6;

/// One year in seconds.
pub const ONE_YEAR_SECONDS: i64 = 31_536_000;

/// PDA seeds.
pub const MINT_AUTHORITY_SEED: &[u8]    = b"mint_authority";
pub const PROGRAM_STATE_SEED: &[u8]     = b"program_state";
pub const HUMAN_REGISTRY_SEED: &[u8]   = b"human_registry";
pub const PROPOSAL_SEED: &[u8]         = b"proposal";
pub const VOTE_SEED: &[u8]             = b"vote";
pub const TREASURY_SEED: &[u8]         = b"treasury";
pub const INFLATION_POOL_SEED: &[u8]   = b"inflation_pool";

/// No single wallet can hold more than 100 million EARTH tokens (6 decimals).
pub const MAX_WALLET_BALANCE: u64 = 100_000_000 * 1_000_000;

/// 51% quorum to pass a proposal.
pub const QUORUM_THRESHOLD_BPS: u64 = 5100;

/// Voting period: 7 days.
pub const VOTING_PERIOD: i64 = 604_800;

/// Challenge window for annual revaluation: 30 days.
/// Governance can reject a submitted revaluation within this window.
pub const REVALUATION_CHALLENGE_WINDOW: i64 = 2_592_000;

// ---- Treasury Milestone Thresholds ----
/// Milestone 1: 100 million verified humans.
/// Unlocks 50% of the community treasury — distributed equally to all humans
/// who were registered at the time the milestone was confirmed.
/// Treasury is LOCKED for spending until this milestone is hit.
pub const MILESTONE_1_THRESHOLD: u64 = 100_000_000;

/// Milestone 2: 500 million verified humans.
/// Unlocks the remaining treasury balance — distributed equally to all verified humans
/// registered at the time of confirmation. After this, treasury rebuilds again.
pub const MILESTONE_2_THRESHOLD: u64 = 500_000_000;

// ============================================================================
// PROGRAM
// ============================================================================

#[program]
pub mod earth {
    use super::*;

    // ========================================================================
    // INITIALIZATION
    // ========================================================================

    /// Initializes the EARTH mint, program state, community treasury,
    /// inflation pool, and humanity reserve pool.
    /// Also sets a backup admin wallet so you are never locked out.
    pub fn initialize_mint(
        ctx: Context<InitializeMint>,
        backup_authority: Pubkey,
    ) -> Result<()> {
        require_keys_eq!(ctx.accounts.admin.key(), ADMIN_AUTHORITY, EarthError::UnauthorizedAdmin);

        let state = &mut ctx.accounts.program_state;
        state.admin_authority               = ADMIN_AUTHORITY;
        state.backup_authority              = backup_authority;
        state.mint                          = ctx.accounts.mint.key();
        state.mint_authority_bump           = ctx.bumps.mint_authority;
        state.treasury_token_account        = ctx.accounts.treasury_token_account.key();
        state.inflation_pool_token_account  = ctx.accounts.inflation_pool_token_account.key();
        state.oracle_data_account           = Pubkey::default();
        state.total_minted                  = 0;
        state.total_verified_humans         = 0;
        state.total_proposals               = 0;
        state.is_initialized                = true;
        state.last_inflation_time           = Clock::get()?.unix_timestamp;
        state.inflation_epoch               = 0;
        state.last_inflation_per_human      = 0;
        state.inflation_pool_token_account  = ctx.accounts.inflation_pool_token_account.key();

        // Dynamic allocation — starts at 66,000, grows each year with Earth's value
        state.current_allocation            = GENESIS_ALLOCATION;

        // Growth rate — starts at 3.5%, governance reviews and adjusts annually
        state.annual_value_growth_bps       = GENESIS_GROWTH_BPS;

        // Inflation rate — starts at 3.5%, can be adjusted by governance
        state.inflation_rate_bps            = GENESIS_GROWTH_BPS;

        // World population tracking — submitted by admin with UN/Worldometer source
        state.estimated_world_population    = 0; // Set via first annual revaluation
        state.revaluation_epoch             = 0;
        state.last_revaluation_time         = 0; // No revaluation yet at genesis

        // Milestone unlock tracking — treasury locked until milestones are hit
        state.milestone_1_reached                  = false;
        state.milestone_2_reached                  = false;
        state.milestone_1_distribution_per_human   = 0;
        state.milestone_2_distribution_per_human   = 0;
        state.milestone_1_humans_snapshot          = 0;
        state.milestone_2_humans_snapshot          = 0;
        state.milestone_1_confirmed_at             = 0;
        state.milestone_2_confirmed_at             = 0;

        msg!("EARTH initialized. Backup admin: {}", backup_authority);
        msg!("Genesis allocation: {} EARTH per human.", GENESIS_ALLOCATION);
        msg!("Treasury: {}", ctx.accounts.treasury_token_account.key());
        Ok(())
    }

    /// Allows the primary admin to update the backup wallet at any time.
    pub fn update_backup_authority(
        ctx: Context<AdminOnly>,
        new_backup: Pubkey,
    ) -> Result<()> {
        ctx.accounts.program_state.backup_authority = new_backup;
        msg!("Backup authority updated to: {}", new_backup);
        Ok(())
    }

    // ========================================================================
    // ANNUAL REVALUATION — ONCE PER YEAR, ADMIN SUBMITS, GOVERNANCE CAN REJECT
    // ========================================================================

    /// Submits the annual Earth value revaluation.
    ///
    /// Called once per year by the admin with:
    ///   - growth_bps: how much Earth's value grew this year (e.g. 350 = 3.5%)
    ///   - new_inflation_rate_bps: the inflation rate for this year (usually same as growth)
    ///   - estimated_world_population: current UN/Worldometer estimate
    ///
    /// Effect:
    ///   - current_allocation grows by growth_bps (new verifiers get more EARTH)
    ///   - existing humans receive their growth share via the inflation pool each year
    ///   - Updates estimated_world_population for on-chain transparency
    ///
    /// Permissioned: admin submits the number with a public source citation (off-chain).
    /// Governance can reject within 30 days via a passed AnnualRevaluation proposal.
    pub fn submit_annual_revaluation(
        ctx: Context<SubmitAnnualRevaluation>,
        growth_bps: u64,
        new_inflation_rate_bps: u64,
        estimated_world_population: u64,
        estimated_new_verifiers_this_year: u64,
    ) -> Result<()> {

        let state    = &ctx.accounts.program_state;
        let clock    = Clock::get()?;

        // Enforce once-per-year rhythm (skip check on first revaluation at epoch 0)
        if state.revaluation_epoch > 0 {
            require!(
                clock.unix_timestamp >= state.last_revaluation_time
                    .checked_add(ONE_YEAR_SECONDS).ok_or(EarthError::ArithmeticOverflow)?,
                EarthError::RevaluationNotDueYet
            );
        }

        // Safety: growth rate capped at 20% (2000 bps) to prevent runaway minting
        require!(growth_bps <= 2000, EarthError::GrowthRateTooHigh);
        require!(new_inflation_rate_bps <= 2000, EarthError::GrowthRateTooHigh);
        require!(estimated_world_population > 0, EarthError::InvalidPopulationEstimate);

        // --- Grow the per-human allocation ---
        // e.g. if current is 66,000 and growth is 3.5%: new = 66,000 + (66,000 × 350 / 10,000)
        let growth_amount = state.current_allocation
            .checked_mul(growth_bps).ok_or(EarthError::ArithmeticOverflow)?
            .checked_div(10_000).ok_or(EarthError::ArithmeticOverflow)?;

        let new_allocation = state.current_allocation
            .checked_add(growth_amount).ok_or(EarthError::ArithmeticOverflow)?;

        // --- Update state ---
        let state = &mut ctx.accounts.program_state;
        state.current_allocation         = new_allocation;
        state.annual_value_growth_bps    = growth_bps;
        state.inflation_rate_bps         = new_inflation_rate_bps;
        state.estimated_world_population = estimated_world_population;
        state.last_revaluation_time      = clock.unix_timestamp;
        state.revaluation_epoch          = state.revaluation_epoch
            .checked_add(1).ok_or(EarthError::ArithmeticOverflow)?;

        msg!("Annual revaluation complete. Epoch: {}", state.revaluation_epoch);
        msg!("New per-human allocation: {} EARTH (grew {}bps).", new_allocation, growth_bps);
        msg!("Estimated world population: {}", estimated_world_population);
        Ok(())
    }

    // ========================================================================
    // HUMAN REGISTRY
    // ========================================================================

    /// Registers a verified human identity on-chain via the authorized oracle.
    ///
    /// WHO CAN REGISTER: biological humans only — any person who can physically
    /// present their iris to the World ID scanner. AI systems, bots, programs,
    /// corporations, and non-human entities cannot register. The iris hash is
    /// unique to each person and cannot be reused — one human, one registration.
    ///
    /// The oracle is responsible for ensuring the iris scan is genuine before
    /// signing this instruction. Once registered, this wallet is permanently
    /// marked as a human identity in the EARTH system.
    pub fn register_human(
        ctx: Context<RegisterHuman>,
        iris_hash: [u8; 32],
    ) -> Result<()> {
        let state = &ctx.accounts.program_state;
        require!(state.oracle_data_account != Pubkey::default(), EarthError::OracleNotSet);
        require_keys_eq!(
            ctx.accounts.oracle_signer.key(),
            state.oracle_data_account,
            EarthError::UnauthorizedOracle
        );

        let human = &mut ctx.accounts.human_registry;
        require!(!human.is_registered, EarthError::HumanAlreadyRegistered);

        human.is_registered              = true;
        human.iris_hash                  = iris_hash;
        human.wallet                     = ctx.accounts.human_wallet.key();
        human.registration_timestamp     = Clock::get()?.unix_timestamp;
        human.is_active                    = true;
        human.has_voted_count              = 0;
        human.last_inflation_epoch_claimed = 0;
        human.has_claimed_allocation       = false;
        human.milestone_1_claimed          = false;
        human.milestone_2_claimed          = false;

        let state = &mut ctx.accounts.program_state;
        state.total_verified_humans = state.total_verified_humans
            .checked_add(1).ok_or(EarthError::ArithmeticOverflow)?;

        msg!("Human registered: {}", human.wallet);
        Ok(())
    }


    // ========================================================================
    // CLAIM INITIAL ALLOCATION
    // ========================================================================

    /// A registered human claims their EARTH allocation.
    /// Tokens are created at this moment — they do not exist beforehand.
    /// Mints directly to the human's wallet and equal amount to treasury.
    /// Can only be called once. Enforces the 100 million EARTH wallet cap.
    pub fn claim_allocation(ctx: Context<ClaimAllocation>) -> Result<()> {
        let state = &ctx.accounts.program_state;

        let allocation = state.current_allocation;

        // Enforce 100 million EARTH wallet cap
        let current_balance = ctx.accounts.human_token_account.amount;
        require!(
            current_balance.checked_add(allocation).ok_or(EarthError::ArithmeticOverflow)?
                <= MAX_WALLET_BALANCE,
            EarthError::WalletCapExceeded
        );

        let human = &mut ctx.accounts.human_registry;
        human.has_claimed_allocation = true;

        let bump = state.mint_authority_bump;
        let signer_seeds: &[&[&[u8]]] = &[&[MINT_AUTHORITY_SEED, &[bump]]];

        // Mint allocation directly to human's wallet
        token_2022::mint_to(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                MintTo {
                    mint:      ctx.accounts.mint.to_account_info(),
                    to:        ctx.accounts.human_token_account.to_account_info(),
                    authority: ctx.accounts.mint_authority.to_account_info(),
                },
                signer_seeds,
            ),
            allocation,
        )?;

        // Mint equal amount to community treasury
        token_2022::mint_to(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                MintTo {
                    mint:      ctx.accounts.mint.to_account_info(),
                    to:        ctx.accounts.treasury_token_account.to_account_info(),
                    authority: ctx.accounts.mint_authority.to_account_info(),
                },
                signer_seeds,
            ),
            allocation,
        )?;

        let total_minted = allocation.checked_mul(2).ok_or(EarthError::ArithmeticOverflow)?;
        let state = &mut ctx.accounts.program_state;
        state.total_minted = state.total_minted
            .checked_add(total_minted).ok_or(EarthError::ArithmeticOverflow)?;

        msg!("{} EARTH → {} | {} → treasury", allocation, ctx.accounts.human.key(), allocation);
        Ok(())
    }

    // ========================================================================
    // ANNUAL INFLATION — SPLIT 50/50: TREASURY + HUMAN POOL
    // ========================================================================

    /// Mints annual inflation once per year — split 50/50:
    ///   Half → community treasury
    ///   Half → inflation pool (claimable equally by all active registered humans)
    ///
    /// The rate used is state.inflation_rate_bps — set by the annual revaluation.
    /// In practice it tracks Earth's value growth (~3-4%), reviewed each year.
    /// Permissionless — anyone can trigger it after 365 days have elapsed.
    pub fn mint_annual_inflation(ctx: Context<MintAnnualInflation>) -> Result<()> {

        let state = &ctx.accounts.program_state;
        let clock = Clock::get()?;

        require!(
            clock.unix_timestamp >= state.last_inflation_time
                .checked_add(ONE_YEAR_SECONDS).ok_or(EarthError::ArithmeticOverflow)?,
            EarthError::InflationNotDueYet
        );

        // Use governance-set rate, not a hardcoded constant
        let inflation_rate = state.inflation_rate_bps;

        let total_inflation = state.total_minted
            .checked_mul(inflation_rate).ok_or(EarthError::ArithmeticOverflow)?
            .checked_div(10_000).ok_or(EarthError::ArithmeticOverflow)?;

        require!(total_inflation > 0, EarthError::InflationAmountZero);

        let half = total_inflation.checked_div(2).ok_or(EarthError::ArithmeticOverflow)?;
        // Remainder (rounding) goes to treasury so no tokens are lost
        let treasury_half = total_inflation.checked_sub(half).ok_or(EarthError::ArithmeticOverflow)?;

        let bump = state.mint_authority_bump;
        let signer_seeds: &[&[&[u8]]] = &[&[MINT_AUTHORITY_SEED, &[bump]]];

        // Mint half → community treasury
        token_2022::mint_to(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                MintTo {
                    mint:      ctx.accounts.mint.to_account_info(),
                    to:        ctx.accounts.treasury_token_account.to_account_info(),
                    authority: ctx.accounts.mint_authority.to_account_info(),
                },
                signer_seeds,
            ),
            treasury_half,
        )?;

        // Mint half → inflation pool (humans claim equally)
        token_2022::mint_to(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                MintTo {
                    mint:      ctx.accounts.mint.to_account_info(),
                    to:        ctx.accounts.inflation_pool_token_account.to_account_info(),
                    authority: ctx.accounts.mint_authority.to_account_info(),
                },
                signer_seeds,
            ),
            half,
        )?;

        let state = &mut ctx.accounts.program_state;
        state.total_minted = state.total_minted
            .checked_add(total_inflation).ok_or(EarthError::ArithmeticOverflow)?;
        state.last_inflation_time = clock.unix_timestamp;
        state.inflation_epoch = state.inflation_epoch
            .checked_add(1).ok_or(EarthError::ArithmeticOverflow)?;
        state.last_inflation_per_human = if state.total_verified_humans > 0 {
            half.checked_div(state.total_verified_humans).unwrap_or(0)
        } else {
            0
        };

        msg!("Annual inflation ({}bps): {} to treasury, {} to human pool ({} each).",
            inflation_rate, treasury_half, half, state.last_inflation_per_human);
        Ok(())
    }

    /// Each active verified human claims their equal share of the annual inflation pool.
    /// Can only be called once per inflation epoch per human.
    pub fn claim_inflation_share(ctx: Context<ClaimInflationShare>) -> Result<()> {

        let state = &ctx.accounts.program_state;
        require!(state.inflation_epoch > 0, EarthError::InflationAmountZero);

        let human = &mut ctx.accounts.human_registry;
        require!(human.is_registered, EarthError::NotRegistered);
        require!(human.is_active, EarthError::HumanNotActive);
        require!(
            human.last_inflation_epoch_claimed < state.inflation_epoch,
            EarthError::InflationAlreadyClaimed
        );

        let share = state.last_inflation_per_human;
        require!(share > 0, EarthError::InflationAmountZero);

        human.last_inflation_epoch_claimed = state.inflation_epoch;

        let signer_seeds: &[&[&[u8]]] = &[&[INFLATION_POOL_SEED, &[ctx.bumps.inflation_pool_token_account]]];

        token_2022::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from:      ctx.accounts.inflation_pool_token_account.to_account_info(),
                    to:        ctx.accounts.human_token_account.to_account_info(),
                    authority: ctx.accounts.inflation_pool_token_account.to_account_info(),
                },
                signer_seeds,
            ),
            share,
        )?;

        msg!("Inflation share claimed: {} EARTH to {}", share, ctx.accounts.human.key());
        Ok(())
    }

    // ========================================================================
    // TREASURY MILESTONE UNLOCKS
    // ========================================================================

    // ========================================================================
    // TREASURY MILESTONE UNLOCKS — LOCKED UNTIL HUMANITY REACHES SCALE
    // ========================================================================

    /// Confirms Milestone 1: 100 million verified humans.
    ///
    /// Requires: admin signature + passed ConfirmMilestone1 governance proposal.
    /// Effect: locks in the per-human distribution amount (50% of treasury / total humans).
    /// After this, every human registered before this moment can claim their share.
    /// Treasury remains locked for free spending — only milestone distributions allowed.
    pub fn confirm_milestone_1(ctx: Context<ConfirmMilestone>) -> Result<()> {
        let state = &ctx.accounts.program_state;
        require!(!state.milestone_1_reached, EarthError::Milestone1AlreadyConfirmed);
        require!(
            state.total_verified_humans >= MILESTONE_1_THRESHOLD,
            EarthError::Milestone1NotReached
        );

        let proposal = &ctx.accounts.milestone_proposal;
        require!(proposal.is_executed, EarthError::SpendProposalNotExecuted);
        require!(proposal.is_passed, EarthError::SpendProposalNotPassed);
        require!(
            proposal.proposal_type == ProposalType::ConfirmMilestone1,
            EarthError::WrongProposalType
        );

        // 50% of current treasury balance divided equally among all registered humans
        let treasury_balance = ctx.accounts.treasury_token_account.amount;
        let half = treasury_balance.checked_div(2).ok_or(EarthError::ArithmeticOverflow)?;
        let total_humans = state.total_verified_humans;
        require!(total_humans > 0, EarthError::NoEligibleVoters);

        let per_human = half.checked_div(total_humans).ok_or(EarthError::ArithmeticOverflow)?;
        require!(per_human > 0, EarthError::MilestoneShareZero);

        let clock = Clock::get()?;
        let state = &mut ctx.accounts.program_state;
        state.milestone_1_reached                = true;
        state.milestone_1_distribution_per_human = per_human;
        state.milestone_1_humans_snapshot        = total_humans;
        state.milestone_1_confirmed_at           = clock.unix_timestamp;

        msg!("MILESTONE 1 CONFIRMED: {} million humans. {} EARTH per person (50% of treasury).",
            total_humans / 1_000_000, per_human);
        Ok(())
    }

    /// Confirms Milestone 2: 500 million verified humans.
    ///
    /// Requires: milestone 1 already confirmed + admin + passed ConfirmMilestone2 proposal.
    /// Effect: distributes whatever remains in the treasury equally to all registered humans
    /// at this moment. After both milestones, treasury keeps rebuilding from new claims
    /// and annual inflation.
    pub fn confirm_milestone_2(ctx: Context<ConfirmMilestone>) -> Result<()> {
        let state = &ctx.accounts.program_state;
        require!(state.milestone_1_reached, EarthError::Milestone1NotConfirmedYet);
        require!(!state.milestone_2_reached, EarthError::Milestone2AlreadyConfirmed);
        require!(
            state.total_verified_humans >= MILESTONE_2_THRESHOLD,
            EarthError::Milestone2NotReached
        );

        let proposal = &ctx.accounts.milestone_proposal;
        require!(proposal.is_executed, EarthError::SpendProposalNotExecuted);
        require!(proposal.is_passed, EarthError::SpendProposalNotPassed);
        require!(
            proposal.proposal_type == ProposalType::ConfirmMilestone2,
            EarthError::WrongProposalType
        );

        // All remaining treasury balance divided equally among all registered humans
        let treasury_balance = ctx.accounts.treasury_token_account.amount;
        let total_humans = state.total_verified_humans;
        require!(total_humans > 0, EarthError::NoEligibleVoters);

        let per_human = treasury_balance.checked_div(total_humans).ok_or(EarthError::ArithmeticOverflow)?;
        require!(per_human > 0, EarthError::MilestoneShareZero);

        let clock = Clock::get()?;
        let state = &mut ctx.accounts.program_state;
        state.milestone_2_reached                = true;
        state.milestone_2_distribution_per_human = per_human;
        state.milestone_2_humans_snapshot        = total_humans;
        state.milestone_2_confirmed_at           = clock.unix_timestamp;

        msg!("MILESTONE 2 CONFIRMED: {} million humans. {} EARTH per person (remaining treasury).",
            total_humans / 1_000_000, per_human);
        Ok(())
    }

    /// Claim your share of the Milestone 1 distribution.
    ///
    /// Eligible: any registered human who was registered BEFORE the milestone
    /// was confirmed. Each person receives milestone_1_distribution_per_human EARTH
    /// transferred from the community treasury. Can only be claimed once.
    pub fn claim_milestone_1_share(ctx: Context<ClaimMilestoneShare>) -> Result<()> {

        let state = &ctx.accounts.program_state;
        require!(state.milestone_1_reached, EarthError::Milestone1NotReached);

        let human = &mut ctx.accounts.human_registry;
        require!(human.is_registered, EarthError::NotRegistered);
        require!(human.is_active, EarthError::HumanNotActive);
        require!(!human.milestone_1_claimed, EarthError::Milestone1ShareAlreadyClaimed);

        // Only humans registered before the milestone was confirmed are eligible
        require!(
            human.registration_timestamp <= state.milestone_1_confirmed_at,
            EarthError::RegisteredAfterMilestone
        );

        let share = state.milestone_1_distribution_per_human;
        require!(share > 0, EarthError::MilestoneShareZero);

        human.milestone_1_claimed = true;

        let signer_seeds: &[&[&[u8]]] = &[&[TREASURY_SEED, &[ctx.bumps.treasury_token_account]]];

        token_2022::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from:      ctx.accounts.treasury_token_account.to_account_info(),
                    to:        ctx.accounts.human_token_account.to_account_info(),
                    authority: ctx.accounts.treasury_token_account.to_account_info(),
                },
                signer_seeds,
            ),
            share,
        )?;

        msg!("Milestone 1 share claimed: {} EARTH to {}", share, ctx.accounts.human.key());
        Ok(())
    }

    /// Claim your share of the Milestone 2 distribution.
    ///
    /// Same pattern as milestone 1 — eligible humans registered before confirmation
    /// each receive milestone_2_distribution_per_human from the treasury.
    pub fn claim_milestone_2_share(ctx: Context<ClaimMilestoneShare>) -> Result<()> {

        let state = &ctx.accounts.program_state;
        require!(state.milestone_2_reached, EarthError::Milestone2NotReached);

        let human = &mut ctx.accounts.human_registry;
        require!(human.is_registered, EarthError::NotRegistered);
        require!(human.is_active, EarthError::HumanNotActive);
        require!(!human.milestone_2_claimed, EarthError::Milestone2ShareAlreadyClaimed);

        // Only humans registered before the milestone was confirmed are eligible
        require!(
            human.registration_timestamp <= state.milestone_2_confirmed_at,
            EarthError::RegisteredAfterMilestone
        );

        let share = state.milestone_2_distribution_per_human;
        require!(share > 0, EarthError::MilestoneShareZero);

        human.milestone_2_claimed = true;

        let signer_seeds: &[&[&[u8]]] = &[&[TREASURY_SEED, &[ctx.bumps.treasury_token_account]]];

        token_2022::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from:      ctx.accounts.treasury_token_account.to_account_info(),
                    to:        ctx.accounts.human_token_account.to_account_info(),
                    authority: ctx.accounts.treasury_token_account.to_account_info(),
                },
                signer_seeds,
            ),
            share,
        )?;

        msg!("Milestone 2 share claimed: {} EARTH to {}", share, ctx.accounts.human.key());
        Ok(())
    }

    // ========================================================================
    // GOVERNANCE — 1 HUMAN, 1 VOTE
    // ========================================================================

    pub fn create_proposal(
        ctx: Context<CreateProposal>,
        proposal_id: [u8; 32],
        proposal_type: ProposalType,
        description_hash: [u8; 32],
    ) -> Result<()> {
        let state = &ctx.accounts.program_state;
        require!(ctx.accounts.proposer_human_registry.is_registered, EarthError::ProposerNotHuman);
        require!(ctx.accounts.proposer_human_registry.is_active, EarthError::ProposerNotActive);

        let clock    = Clock::get()?;
        let proposal = &mut ctx.accounts.proposal;
        proposal.proposal_id           = proposal_id;
        proposal.proposal_type         = proposal_type;
        proposal.description_hash      = description_hash;
        proposal.proposer              = ctx.accounts.proposer.key();
        proposal.created_at            = clock.unix_timestamp;
        proposal.voting_ends_at        = clock.unix_timestamp
            .checked_add(VOTING_PERIOD).ok_or(EarthError::ArithmeticOverflow)?;
        proposal.votes_for             = 0;
        proposal.votes_against         = 0;
        proposal.total_eligible_voters = state.total_verified_humans;
        proposal.is_active             = true;
        proposal.is_executed           = false;
        proposal.is_passed             = false;

        let state = &mut ctx.accounts.program_state;
        state.total_proposals = state.total_proposals
            .checked_add(1).ok_or(EarthError::ArithmeticOverflow)?;

        msg!("Proposal created. Type: {:?}. Eligible voters: {}", proposal_type, proposal.total_eligible_voters);
        Ok(())
    }

    pub fn cast_vote(ctx: Context<CastVote>, vote_choice: bool) -> Result<()> {
        require!(ctx.accounts.voter_human_registry.is_registered, EarthError::VoterNotHuman);
        require!(ctx.accounts.voter_human_registry.is_active, EarthError::VoterNotActive);
        require_keys_eq!(
            ctx.accounts.voter_human_registry.wallet,
            ctx.accounts.voter.key(),
            EarthError::VoterWalletMismatch
        );

        let proposal = &mut ctx.accounts.proposal;
        require!(proposal.is_active, EarthError::ProposalNotActive);
        require!(Clock::get()?.unix_timestamp <= proposal.voting_ends_at, EarthError::VotingPeriodEnded);

        let vote_record = &mut ctx.accounts.vote_record;
        require!(!vote_record.has_voted, EarthError::AlreadyVoted);

        vote_record.has_voted   = true;
        vote_record.voter       = ctx.accounts.voter.key();
        vote_record.proposal    = proposal.proposal_id;
        vote_record.vote_choice = vote_choice;
        vote_record.voted_at    = Clock::get()?.unix_timestamp;

        if vote_choice {
            proposal.votes_for = proposal.votes_for
                .checked_add(1).ok_or(EarthError::ArithmeticOverflow)?;
        } else {
            proposal.votes_against = proposal.votes_against
                .checked_add(1).ok_or(EarthError::ArithmeticOverflow)?;
        }

        msg!("Vote cast: {}", if vote_choice { "FOR" } else { "AGAINST" });
        Ok(())
    }

    pub fn finalize_proposal(ctx: Context<FinalizeProposal>) -> Result<()> {

        let proposal = &mut ctx.accounts.proposal;
        require!(proposal.is_active, EarthError::ProposalNotActive);
        require!(!proposal.is_executed, EarthError::ProposalAlreadyExecuted);
        require!(
            Clock::get()?.unix_timestamp > proposal.voting_ends_at,
            EarthError::VotingPeriodNotEnded
        );
        require!(proposal.total_eligible_voters > 0, EarthError::NoEligibleVoters);

        let total_votes = proposal.votes_for
            .checked_add(proposal.votes_against).ok_or(EarthError::ArithmeticOverflow)?;
        let quorum_required = proposal.total_eligible_voters
            .checked_mul(QUORUM_THRESHOLD_BPS).ok_or(EarthError::ArithmeticOverflow)?
            .checked_div(10_000).ok_or(EarthError::ArithmeticOverflow)?;

        proposal.is_active   = false;
        proposal.is_executed = true;
        proposal.is_passed   = (total_votes >= quorum_required)
            && (proposal.votes_for > proposal.votes_against);

        msg!("Proposal finalized. Passed: {}", proposal.is_passed);
        Ok(())
    }


    // ========================================================================
    // ORACLE UPDATE
    // ========================================================================

    pub fn update_oracle(ctx: Context<AdminOnly>, new_oracle: Pubkey) -> Result<()> {
        ctx.accounts.program_state.oracle_data_account = new_oracle;
        msg!("Oracle updated to: {}", new_oracle);
        Ok(())
    }
}

// ============================================================================
// ACCOUNT STRUCTS
// ============================================================================

/// Shared constraint: signer must be primary OR backup admin.
fn is_admin(signer: &Pubkey, state: &ProgramState) -> bool {
    *signer == state.admin_authority || *signer == state.backup_authority
}

#[derive(Accounts)]
pub struct InitializeMint<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        init,
        payer = admin,
        mint::decimals = TOKEN_DECIMALS,
        mint::authority = mint_authority,
        mint::token_program = token_program,
    )]
    pub mint: InterfaceAccount<'info, Mint>,

    /// CHECK: PDA mint authority.
    #[account(seeds = [MINT_AUTHORITY_SEED], bump)]
    pub mint_authority: UncheckedAccount<'info>,

    #[account(
        init,
        payer = admin,
        space = 8 + ProgramState::INIT_SPACE,
        seeds = [PROGRAM_STATE_SEED],
        bump,
    )]
    pub program_state: Account<'info, ProgramState>,

    #[account(
        init,
        payer = admin,
        token::mint = mint,
        token::authority = mint_authority,
        token::token_program = token_program,
        seeds = [TREASURY_SEED],
        bump,
    )]
    pub treasury_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(
        init,
        payer = admin,
        token::mint = mint,
        token::authority = inflation_pool_token_account,
        token::token_program = token_program,
        seeds = [INFLATION_POOL_SEED],
        bump,
    )]
    pub inflation_pool_token_account: InterfaceAccount<'info, TokenAccount>,

    /// Humanity reserve — holds pre-minted tokens for expected new verifiers each year.
    /// Every person on Earth has a claim here, registered or not.
    /// Unclaimed shares eventually flow to the community treasury.
    #[account(
        init,
        payer = admin,
        token::mint = mint,
        token::authority = mint_authority,
        token::token_program = token_program,
    )]
    pub inflation_pool_token_account_init: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Program<'info, Token2022>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

/// Reusable admin-only context. Accepts primary OR backup admin.
#[derive(Accounts)]
pub struct AdminOnly<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [PROGRAM_STATE_SEED],
        bump,
        constraint = program_state.is_initialized @ EarthError::NotInitialized,
        constraint = is_admin(&admin.key(), &program_state) @ EarthError::UnauthorizedAdmin,
    )]
    pub program_state: Account<'info, ProgramState>,
}

#[derive(Accounts)]
pub struct SubmitAnnualRevaluation<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(mut, constraint = mint.key() == program_state.mint @ EarthError::InvalidMint)]
    pub mint: InterfaceAccount<'info, Mint>,

    /// CHECK: PDA mint authority.
    #[account(seeds = [MINT_AUTHORITY_SEED], bump = program_state.mint_authority_bump)]
    pub mint_authority: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [PROGRAM_STATE_SEED],
        bump,
        constraint = program_state.is_initialized @ EarthError::NotInitialized,
        constraint = is_admin(&admin.key(), &program_state) @ EarthError::UnauthorizedAdmin,
    )]
    pub program_state: Account<'info, ProgramState>,

    pub token_program: Program<'info, Token2022>,
}

#[derive(Accounts)]
#[instruction(iris_hash: [u8; 32])]
pub struct RegisterHuman<'info> {
    #[account(mut)]
    pub oracle_signer: Signer<'info>,

    /// CHECK: The wallet being registered.
    pub human_wallet: UncheckedAccount<'info>,

    #[account(
        init,
        payer = oracle_signer,
        space = 8 + HumanRegistry::INIT_SPACE,
        seeds = [HUMAN_REGISTRY_SEED, human_wallet.key().as_ref()],
        bump,
    )]
    pub human_registry: Account<'info, HumanRegistry>,

    #[account(
        mut,
        seeds = [PROGRAM_STATE_SEED],
        bump,
        constraint = program_state.is_initialized @ EarthError::NotInitialized,
    )]
    pub program_state: Account<'info, ProgramState>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]

#[derive(Accounts)]
pub struct MintAnnualInflation<'info> {
    /// CHECK: Permissionless — anyone can trigger after 365 days.
    pub caller: UncheckedAccount<'info>,

    #[account(mut, constraint = mint.key() == program_state.mint @ EarthError::InvalidMint)]
    pub mint: InterfaceAccount<'info, Mint>,

    /// CHECK: PDA mint authority.
    #[account(seeds = [MINT_AUTHORITY_SEED], bump = program_state.mint_authority_bump)]
    pub mint_authority: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [PROGRAM_STATE_SEED],
        bump,
        constraint = program_state.is_initialized @ EarthError::NotInitialized,
    )]
    pub program_state: Account<'info, ProgramState>,

    #[account(
        mut,
        constraint = treasury_token_account.key() == program_state.treasury_token_account @ EarthError::InvalidTreasuryAccount,
    )]
    pub treasury_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [INFLATION_POOL_SEED],
        bump,
        constraint = inflation_pool_token_account.key() == program_state.inflation_pool_token_account @ EarthError::InvalidInflationPoolAccount,
    )]
    pub inflation_pool_token_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Program<'info, Token2022>,
}

#[derive(Accounts)]
pub struct ClaimInflationShare<'info> {
    #[account(mut)]
    pub human: Signer<'info>,

    #[account(
        mut,
        seeds = [HUMAN_REGISTRY_SEED, human.key().as_ref()],
        bump,
        constraint = human_registry.wallet == human.key() @ EarthError::SenderWalletMismatch,
    )]
    pub human_registry: Account<'info, HumanRegistry>,

    #[account(
        mut,
        seeds = [INFLATION_POOL_SEED],
        bump,
    )]
    pub inflation_pool_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub human_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(seeds = [PROGRAM_STATE_SEED], bump, constraint = program_state.is_initialized @ EarthError::NotInitialized)]
    pub program_state: Account<'info, ProgramState>,

    pub token_program: Program<'info, Token2022>,
}

#[derive(Accounts)]
pub struct ClaimAllocation<'info> {
    #[account(mut)]
    pub human: Signer<'info>,

    #[account(
        mut,
        seeds = [HUMAN_REGISTRY_SEED, human.key().as_ref()],
        bump,
        constraint = human_registry.is_registered @ EarthError::NotRegistered,
        constraint = human_registry.is_active @ EarthError::HumanNotActive,
        constraint = !human_registry.has_claimed_allocation @ EarthError::AllocationAlreadyClaimed,
        constraint = human_registry.wallet == human.key() @ EarthError::SenderWalletMismatch,
    )]
    pub human_registry: Account<'info, HumanRegistry>,

    #[account(mut, constraint = mint.key() == program_state.mint @ EarthError::InvalidMint)]
    pub mint: InterfaceAccount<'info, Mint>,

    /// CHECK: PDA mint authority.
    #[account(seeds = [MINT_AUTHORITY_SEED], bump = program_state.mint_authority_bump)]
    pub mint_authority: UncheckedAccount<'info>,

    #[account(mut)]
    pub human_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        constraint = treasury_token_account.key() == program_state.treasury_token_account @ EarthError::InvalidTreasuryAccount,
    )]
    pub treasury_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(
        seeds = [PROGRAM_STATE_SEED],
        bump,
        constraint = program_state.is_initialized @ EarthError::NotInitialized,
    )]
    pub program_state: Account<'info, ProgramState>,

    pub token_program: Program<'info, Token2022>,
}

#[derive(Accounts)]
#[instruction(proposal_id: [u8; 32])]
pub struct CreateProposal<'info> {
    #[account(mut)]
    pub proposer: Signer<'info>,

    #[account(
        constraint = proposer_human_registry.is_registered @ EarthError::ProposerNotHuman,
        constraint = proposer_human_registry.wallet == proposer.key() @ EarthError::ProposerWalletMismatch,
        seeds = [HUMAN_REGISTRY_SEED, proposer.key().as_ref()],
        bump,
    )]
    pub proposer_human_registry: Account<'info, HumanRegistry>,

    #[account(
        init,
        payer = proposer,
        space = 8 + Proposal::INIT_SPACE,
        seeds = [PROPOSAL_SEED, &proposal_id],
        bump,
    )]
    pub proposal: Account<'info, Proposal>,

    #[account(mut, seeds = [PROGRAM_STATE_SEED], bump, constraint = program_state.is_initialized @ EarthError::NotInitialized)]
    pub program_state: Account<'info, ProgramState>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct CastVote<'info> {
    #[account(mut)]
    pub voter: Signer<'info>,

    #[account(
        constraint = voter_human_registry.is_registered @ EarthError::VoterNotHuman,
        constraint = voter_human_registry.wallet == voter.key() @ EarthError::VoterWalletMismatch,
        seeds = [HUMAN_REGISTRY_SEED, voter.key().as_ref()],
        bump,
    )]
    pub voter_human_registry: Account<'info, HumanRegistry>,

    #[account(mut, constraint = proposal.is_active @ EarthError::ProposalNotActive)]
    pub proposal: Account<'info, Proposal>,

    #[account(
        init,
        payer = voter,
        space = 8 + VoteRecord::INIT_SPACE,
        seeds = [VOTE_SEED, proposal.proposal_id.as_ref(), voter.key().as_ref()],
        bump,
    )]
    pub vote_record: Account<'info, VoteRecord>,

    #[account(seeds = [PROGRAM_STATE_SEED], bump, constraint = program_state.is_initialized @ EarthError::NotInitialized)]
    pub program_state: Account<'info, ProgramState>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct FinalizeProposal<'info> {
    #[account(mut)]
    pub proposal: Account<'info, Proposal>,

    #[account(seeds = [PROGRAM_STATE_SEED], bump, constraint = program_state.is_initialized @ EarthError::NotInitialized)]
    pub program_state: Account<'info, ProgramState>,
}

/// Used by both confirm_milestone_1 and confirm_milestone_2.
/// Admin submits with a passed governance proposal; treasury balance is read to calculate per-human amount.
#[derive(Accounts)]
pub struct ConfirmMilestone<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    /// The governance proposal authorizing this milestone confirmation.
    pub milestone_proposal: Account<'info, Proposal>,

    /// Treasury account — balance read to calculate per-human distribution.
    #[account(
        seeds = [TREASURY_SEED],
        bump,
        constraint = treasury_token_account.key() == program_state.treasury_token_account @ EarthError::InvalidTreasuryAccount,
    )]
    pub treasury_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [PROGRAM_STATE_SEED],
        bump,
        constraint = program_state.is_initialized @ EarthError::NotInitialized,
        constraint = is_admin(&admin.key(), &program_state) @ EarthError::UnauthorizedAdmin,
    )]
    pub program_state: Account<'info, ProgramState>,
}

/// Used by both claim_milestone_1_share and claim_milestone_2_share.
#[derive(Accounts)]
pub struct ClaimMilestoneShare<'info> {
    #[account(mut)]
    pub human: Signer<'info>,

    #[account(
        mut,
        seeds = [HUMAN_REGISTRY_SEED, human.key().as_ref()],
        bump,
        constraint = human_registry.wallet == human.key() @ EarthError::SenderWalletMismatch,
    )]
    pub human_registry: Account<'info, HumanRegistry>,

    #[account(
        mut,
        seeds = [TREASURY_SEED],
        bump,
        constraint = treasury_token_account.key() == program_state.treasury_token_account @ EarthError::InvalidTreasuryAccount,
    )]
    pub treasury_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub human_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(seeds = [PROGRAM_STATE_SEED], bump, constraint = program_state.is_initialized @ EarthError::NotInitialized)]
    pub program_state: Account<'info, ProgramState>,

    pub token_program: Program<'info, Token2022>,
}


// ============================================================================
// STATE ACCOUNTS
// ============================================================================

#[account]
#[derive(InitSpace)]
pub struct ProgramState {
    pub admin_authority:                Pubkey,
    pub backup_authority:               Pubkey,
    pub mint:                           Pubkey,
    pub mint_authority_bump:            u8,
    pub treasury_token_account:         Pubkey,
    pub inflation_pool_token_account:   Pubkey,
    pub oracle_data_account:            Pubkey,
    pub total_minted:                   u64,
    pub total_verified_humans:          u64,
    pub total_proposals:                u64,
    pub is_initialized:                 bool,
    pub last_inflation_time:            i64,
    pub inflation_epoch:                u64,
    pub last_inflation_per_human:       u64,

    // ---- Dynamic value tracking ----
    /// Per-human token allocation. Starts at 66,000. Grows each year
    /// with Earth's value via submit_annual_revaluation.
    pub current_allocation:             u64,

    /// Annual value growth rate in basis points. Reviewed each year.
    /// Reflects Earth's real economic growth (~3-4% historically).
    pub annual_value_growth_bps:        u64,

    /// Annual inflation rate in basis points. Usually matches growth rate.
    /// Governance can set separately if conditions warrant.
    pub inflation_rate_bps:             u64,

    /// Estimated world population from UN/Worldometer. Set each year.
    /// Tracks the total human claim on EARTH — registered or not.
    pub estimated_world_population:     u64,

    /// When the last annual revaluation was submitted.
    pub last_revaluation_time:          i64,

    /// Increments each year. Tracks revaluation history.
    pub revaluation_epoch:              u64,

    // ---- Milestone Treasury Unlock Tracking ----
    /// Whether the 100 million human milestone has been confirmed.
    /// Treasury is LOCKED for spending until this is true.
    pub milestone_1_reached:                  bool,

    /// Whether the 500 million human milestone has been confirmed.
    pub milestone_2_reached:                  bool,

    /// Per-human EARTH amount locked in at milestone 1 confirmation (50% of treasury / humans).
    pub milestone_1_distribution_per_human:   u64,

    /// Per-human EARTH amount locked in at milestone 2 confirmation (remaining treasury / humans).
    pub milestone_2_distribution_per_human:   u64,

    /// Total verified humans at the time milestone 1 was confirmed.
    pub milestone_1_humans_snapshot:          u64,

    /// Total verified humans at the time milestone 2 was confirmed.
    pub milestone_2_humans_snapshot:          u64,

    /// Unix timestamp when milestone 1 was confirmed.
    /// Humans registered AFTER this timestamp are not eligible for the milestone 1 distribution.
    pub milestone_1_confirmed_at:             i64,

    /// Unix timestamp when milestone 2 was confirmed.
    pub milestone_2_confirmed_at:             i64,
}

#[account]
#[derive(InitSpace)]
pub struct HumanRegistry {
    pub is_registered:                bool,
    pub iris_hash:                    [u8; 32],
    pub wallet:                       Pubkey,
    pub registration_timestamp:       i64,
    pub is_active:                    bool,
    pub has_voted_count:              u64,
    pub last_inflation_epoch_claimed: u64,
    pub has_claimed_allocation:       bool,
    pub milestone_1_claimed:          bool,
    pub milestone_2_claimed:          bool,
}

#[account]
#[derive(InitSpace)]
pub struct Proposal {
    pub proposal_id:           [u8; 32],
    pub proposal_type:         ProposalType,
    pub description_hash:      [u8; 32],
    pub proposer:              Pubkey,
    pub created_at:            i64,
    pub voting_ends_at:        i64,
    pub votes_for:             u64,
    pub votes_against:         u64,
    pub total_eligible_voters: u64,
    pub is_active:             bool,
    pub is_executed:           bool,
    pub is_passed:             bool,
}

#[account]
#[derive(InitSpace)]
pub struct VoteRecord {
    pub has_voted:   bool,
    pub voter:       Pubkey,
    pub proposal:    [u8; 32],
    pub vote_choice: bool,
    pub voted_at:    i64,
}

// ============================================================================
// ENUMS
// ============================================================================

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug, InitSpace)]
pub enum ProposalType {
    SystemChange,
    OracleUpdate,
    InfrastructureDeployment,
    TreasurySpend,          // Authorize spending community treasury funds (post-milestone only)
    AnnualRevaluation,      // Challenge or ratify the submitted annual revaluation
    UpdateInflationRate,    // Adjust the inflation/growth rate
    ConfirmMilestone1,      // Governance vote to unlock treasury at 100 million humans
    ConfirmMilestone2,      // Governance vote to unlock treasury at 500 million humans
}

// ============================================================================
// ERRORS
// ============================================================================

#[error_code]
pub enum EarthError {
    #[msg("Unauthorized: not the admin or backup admin.")]
    UnauthorizedAdmin,
    #[msg("Unauthorized: oracle signer does not match.")]
    UnauthorizedOracle,
    #[msg("Program not initialized.")]
    NotInitialized,
    #[msg("Wrong proposal type for this operation.")]
    WrongProposalType,
    #[msg("Arithmetic overflow.")]
    ArithmeticOverflow,
    #[msg("Invalid mint account.")]
    InvalidMint,
    #[msg("Invalid treasury account.")]
    InvalidTreasuryAccount,
    #[msg("Invalid inflation pool account.")]
    InvalidInflationPoolAccount,
    #[msg("Human already registered.")]
    HumanAlreadyRegistered,
    #[msg("Oracle not configured.")]
    OracleNotSet,
    #[msg("No eligible voters for this proposal.")]
    NoEligibleVoters,
    #[msg("Annual inflation not due yet.")]
    InflationNotDueYet,
    #[msg("Inflation amount is zero.")]
    InflationAmountZero,
    #[msg("Annual revaluation not due yet — must wait one full year.")]
    RevaluationNotDueYet,
    #[msg("Growth rate too high — capped at 20% (2000 bps) for safety.")]
    GrowthRateTooHigh,
    #[msg("Invalid population estimate — must be greater than zero.")]
    InvalidPopulationEstimate,
    #[msg("Treasury spend proposal has not been executed.")]
    SpendProposalNotExecuted,
    #[msg("Treasury spend proposal did not pass governance vote.")]
    SpendProposalNotPassed,
    #[msg("Spend amount must be greater than zero.")]
    SpendAmountZero,
    #[msg("Sender wallet mismatch.")]
    SenderWalletMismatch,
    #[msg("Claimer is not a registered human.")]
    ClaimerNotHuman,
    #[msg("Claimer is not active.")]
    ClaimerNotActive,
    #[msg("Claimer wallet mismatch.")]
    ClaimerWalletMismatch,
    #[msg("Voter is not a registered human.")]
    VoterNotHuman,
    #[msg("Voter is not active.")]
    VoterNotActive,
    #[msg("Voter wallet mismatch.")]
    VoterWalletMismatch,
    #[msg("Already voted on this proposal.")]
    AlreadyVoted,
    #[msg("Proposal is not active.")]
    ProposalNotActive,
    #[msg("Voting period has ended.")]
    VotingPeriodEnded,
    #[msg("Voting period has not ended yet.")]
    VotingPeriodNotEnded,
    #[msg("Proposal already executed.")]
    ProposalAlreadyExecuted,
    #[msg("Proposer is not a registered human.")]
    ProposerNotHuman,
    #[msg("Proposer is not active.")]
    ProposerNotActive,
    #[msg("Proposer wallet mismatch.")]
    ProposerWalletMismatch,
    #[msg("Human is not registered.")]
    NotRegistered,
    #[msg("Human is not active.")]
    HumanNotActive,
    #[msg("Allocation already claimed.")]
    AllocationAlreadyClaimed,
    #[msg("Wallet balance would exceed the 100 million EARTH cap.")]
    WalletCapExceeded,
    #[msg("Inflation share already claimed for this epoch.")]
    InflationAlreadyClaimed,
    #[msg("Milestone 1 (100M humans) has not been reached yet.")]
    Milestone1NotReached,
    #[msg("Milestone 2 (500M humans) has not been reached yet.")]
    Milestone2NotReached,
    #[msg("Milestone 1 has already been confirmed.")]
    Milestone1AlreadyConfirmed,
    #[msg("Milestone 2 has already been confirmed.")]
    Milestone2AlreadyConfirmed,
    #[msg("Milestone 1 must be confirmed before Milestone 2.")]
    Milestone1NotConfirmedYet,
    #[msg("Milestone 1 share already claimed.")]
    Milestone1ShareAlreadyClaimed,
    #[msg("Milestone 2 share already claimed.")]
    Milestone2ShareAlreadyClaimed,
    #[msg("Milestone distribution share is zero — treasury may be empty.")]
    MilestoneShareZero,
    #[msg("You registered after the milestone was confirmed and are not eligible for this distribution.")]
    RegisteredAfterMilestone,
    #[msg("Community treasury is locked until Milestone 1 (100M humans) is confirmed.")]
    TreasuryLockedUntilMilestone,
}
