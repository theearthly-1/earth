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

/// Tokens minted per verified human (66,000 EARTH, 6 decimals).
pub const BIRTH_ALLOCATION: u64 = 66_000_000_000;

/// Token decimals.
pub const TOKEN_DECIMALS: u8 = 6;

/// 3.5% annual inflation in basis points.
pub const INFLATION_BPS: u64 = 350;

/// One year in seconds.
pub const ONE_YEAR_SECONDS: i64 = 31_536_000;

/// PDA seeds.
pub const MINT_AUTHORITY_SEED: &[u8] = b"mint_authority";
pub const PROGRAM_STATE_SEED: &[u8]  = b"program_state";
pub const VAULT_SEED: &[u8]          = b"vault";
pub const HUMAN_REGISTRY_SEED: &[u8] = b"human_registry";
pub const PROPOSAL_SEED: &[u8]       = b"proposal";
pub const VOTE_SEED: &[u8]           = b"vote";
pub const TREASURY_SEED: &[u8]       = b"treasury";
pub const INFLATION_POOL_SEED: &[u8] = b"inflation_pool";

/// 51% quorum to pass a proposal.
pub const QUORUM_THRESHOLD_BPS: u64 = 5100;

/// Voting period: 7 days.
pub const VOTING_PERIOD: i64 = 604_800;

// ============================================================================
// PROGRAM
// ============================================================================

#[program]
pub mod earth {
    use super::*;

    // ========================================================================
    // INITIALIZATION
    // ========================================================================

    /// Initializes the EARTH mint, program state, and community treasury.
    /// Also sets a backup admin wallet so you are never locked out.
    pub fn initialize_mint(
        ctx: Context<InitializeMint>,
        backup_authority: Pubkey, // Pass your second/backup wallet address here
    ) -> Result<()> {
        require_keys_eq!(ctx.accounts.admin.key(), ADMIN_AUTHORITY, EarthError::UnauthorizedAdmin);

        let state = &mut ctx.accounts.program_state;
        state.admin_authority        = ADMIN_AUTHORITY;
        state.backup_authority       = backup_authority;
        state.mint                   = ctx.accounts.mint.key();
        state.mint_authority_bump    = ctx.bumps.mint_authority;
        state.treasury_token_account = ctx.accounts.treasury_token_account.key();
        state.oracle_data_account    = Pubkey::default();
        state.total_minted           = 0;
        state.total_birth_events     = 0;
        state.total_verified_humans  = 0;
        state.total_proposals        = 0;
        state.is_initialized                  = true;
        state.emergency_freeze               = false;
        state.freeze_reason                  = [0u8; 64];
        state.freeze_timestamp               = 0;
        state.last_inflation_time            = Clock::get()?.unix_timestamp;
        state.inflation_epoch                = 0;
        state.last_inflation_per_human       = 0;
        state.inflation_pool_token_account   = ctx.accounts.inflation_pool_token_account.key();

        msg!("EARTH initialized. Backup admin: {}", backup_authority);
        msg!("Treasury: {}", ctx.accounts.treasury_token_account.key());
        Ok(())
    }

    /// Allows the primary admin to update the backup wallet at any time.
    /// Use this if you get a new secondary wallet.
    pub fn update_backup_authority(
        ctx: Context<AdminOnly>,
        new_backup: Pubkey,
    ) -> Result<()> {
        ctx.accounts.program_state.backup_authority = new_backup;
        msg!("Backup authority updated to: {}", new_backup);
        Ok(())
    }

    // ========================================================================
    // HUMAN REGISTRY
    // ========================================================================

    /// Registers a verified human identity on-chain via the authorized oracle.
    pub fn register_human(
        ctx: Context<RegisterHuman>,
        iris_hash: [u8; 32],
    ) -> Result<()> {
        let state = &ctx.accounts.program_state;
        require!(!state.emergency_freeze, EarthError::SystemFrozen);
        require!(state.oracle_data_account != Pubkey::default(), EarthError::OracleNotSet);
        require_keys_eq!(
            ctx.accounts.oracle_signer.key(),
            state.oracle_data_account,
            EarthError::UnauthorizedOracle
        );

        let human = &mut ctx.accounts.human_registry;
        require!(!human.is_registered, EarthError::HumanAlreadyRegistered);

        human.is_registered          = true;
        human.iris_hash              = iris_hash;
        human.wallet                 = ctx.accounts.human_wallet.key();
        human.registration_timestamp = Clock::get()?.unix_timestamp;
        human.is_active              = true;
        human.has_voted_count        = 0;
        human.heir                        = Pubkey::default(); // No heir set by default
        human.is_deceased                = false;
        human.last_inflation_epoch_claimed = 0;

        let state = &mut ctx.accounts.program_state;
        state.total_verified_humans = state.total_verified_humans
            .checked_add(1).ok_or(EarthError::ArithmeticOverflow)?;

        msg!("Human registered: {}", human.wallet);
        Ok(())
    }

    // ========================================================================
    // HEIR DESIGNATION — SET BEFORE YOU DIE
    // ========================================================================

    /// A verified human sets their heir — another verified human wallet.
    /// If they die with an unclaimed vault and an heir is set,
    /// the vault transfers to the heir instead of the community treasury.
    /// Set heir to Pubkey::default() (all zeros) to remove your heir designation.
    pub fn set_heir(
        ctx: Context<SetHeir>,
        heir_wallet: Pubkey,
    ) -> Result<()> {
        require!(!ctx.accounts.program_state.emergency_freeze, EarthError::SystemFrozen);

        let human = &mut ctx.accounts.human_registry;
        require!(human.is_registered, EarthError::NotRegistered);
        require!(human.is_active, EarthError::HumanNotActive);
        require!(!human.is_deceased, EarthError::HumanDeceased);

        // If setting a real heir (not clearing), verify heir is a registered human
        if heir_wallet != Pubkey::default() {
            require!(
                ctx.accounts.heir_registry.is_registered,
                EarthError::HeirNotHuman
            );
            require!(
                ctx.accounts.heir_registry.wallet == heir_wallet,
                EarthError::HeirWalletMismatch
            );
        }

        human.heir = heir_wallet;
        msg!("Heir set to: {}", heir_wallet);
        Ok(())
    }

    // ========================================================================
    // DEATH DECLARATION — ORACLE ONLY
    // ========================================================================

    /// The oracle declares a human deceased.
    /// - Marks their registry as inactive and deceased.
    /// - If they have an unclaimed vault: transfers it to heir (if set) or treasury.
    /// - Tokens already in their personal wallet must be transferred by the person
    ///   before death, or they remain locked until governance decides.
    pub fn declare_deceased(
        ctx: Context<DeclareDeceased>,
        _birth_event_id: [u8; 32], // Used to locate the vault PDA
    ) -> Result<()> {
        let state = &ctx.accounts.program_state;
        require!(!state.emergency_freeze, EarthError::SystemFrozen);
        require_keys_eq!(
            ctx.accounts.oracle_signer.key(),
            state.oracle_data_account,
            EarthError::UnauthorizedOracle
        );

        let human = &mut ctx.accounts.human_registry;
        require!(human.is_registered, EarthError::NotRegistered);
        require!(!human.is_deceased, EarthError::HumanDeceased);

        human.is_active   = false;
        human.is_deceased = true;

        // If vault exists and is unclaimed, redirect it
        let vault = &mut ctx.accounts.vault_state;
        if vault.is_initialized && !vault.is_claimed {
            vault.is_claimed = true; // Mark as processed

            let birth_event_id = vault.birth_event_id;
            let vault_bump     = vault.vault_bump;
            let amount         = vault.amount;
            let heir           = human.heir;

            let signer: &[&[&[u8]]] = &[&[VAULT_SEED, &birth_event_id, &[vault_bump]]];

            // If heir is set and is a verified human → transfer to heir
            // Otherwise → transfer to community treasury
            let destination = ctx.accounts.destination_token_account.to_account_info();

            token_2022::transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    Transfer {
                        from:      ctx.accounts.vault_token_account.to_account_info(),
                        to:        destination,
                        authority: ctx.accounts.vault_state.to_account_info(),
                    },
                    signer,
                ),
                amount,
            )?;

            if heir != Pubkey::default() {
                msg!("Deceased vault transferred to heir: {}", heir);
            } else {
                msg!("Deceased vault returned to community treasury.");
            }
        }

        msg!("Human declared deceased: {}", human.wallet);
        Ok(())
    }

    // ========================================================================
    // MINTING — DUAL ALLOCATION (HUMAN + COMMUNITY POOL)
    // ========================================================================

    /// Mints 66,000 EARTH to the beneficiary vault AND
    /// 66,000 EARTH to the community treasury pool simultaneously.
    pub fn mint_birth_allocation(
        ctx: Context<MintBirthAllocation>,
        birth_event_id: [u8; 32],
        beneficiary: Pubkey,
        is_minor: bool,
        birth_timestamp: i64,
    ) -> Result<()> {
        let state = &ctx.accounts.program_state;
        require!(!state.emergency_freeze, EarthError::SystemFrozen);
        require!(state.oracle_data_account != Pubkey::default(), EarthError::OracleNotSet);
        require_keys_eq!(
            ctx.accounts.oracle_signer.key(),
            state.oracle_data_account,
            EarthError::UnauthorizedOracle
        );

        let vault = &mut ctx.accounts.vault_state;
        require!(!vault.is_initialized, EarthError::BirthEventAlreadyProcessed);

        vault.is_initialized      = true;
        vault.birth_event_id      = birth_event_id;
        vault.beneficiary         = beneficiary;
        vault.is_minor            = is_minor;
        vault.birth_timestamp     = birth_timestamp;
        vault.amount              = BIRTH_ALLOCATION;
        vault.is_claimed          = false;
        vault.vault_token_account = ctx.accounts.vault_token_account.key();
        vault.vault_bump          = ctx.bumps.vault_state;
        vault.unlock_timestamp    = if is_minor {
            birth_timestamp.checked_add(568_036_800).ok_or(EarthError::ArithmeticOverflow)?
        } else {
            0
        };

        let bump = ctx.accounts.program_state.mint_authority_bump;
        let signer_seeds: &[&[&[u8]]] = &[&[MINT_AUTHORITY_SEED, &[bump]]];

        // Mint 66,000 → individual vault
        token_2022::mint_to(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                MintTo {
                    mint:      ctx.accounts.mint.to_account_info(),
                    to:        ctx.accounts.vault_token_account.to_account_info(),
                    authority: ctx.accounts.mint_authority.to_account_info(),
                },
                signer_seeds,
            ),
            BIRTH_ALLOCATION,
        )?;

        // Mint 66,000 → community treasury pool
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
            BIRTH_ALLOCATION,
        )?;

        let total_minted = BIRTH_ALLOCATION.checked_mul(2).ok_or(EarthError::ArithmeticOverflow)?;
        let state = &mut ctx.accounts.program_state;
        state.total_minted = state.total_minted
            .checked_add(total_minted).ok_or(EarthError::ArithmeticOverflow)?;
        state.total_birth_events = state.total_birth_events
            .checked_add(1).ok_or(EarthError::ArithmeticOverflow)?;

        msg!("66,000 → vault | 66,000 → treasury | Beneficiary: {}", beneficiary);
        Ok(())
    }

    // ========================================================================
    // ANNUAL INFLATION — 3.5% PER YEAR TO TREASURY
    // ========================================================================

    /// Mints 3.5% of total supply once per year — split 50/50:
    ///   1.75% → community treasury
    ///   1.75% → inflation pool (claimable equally by all active registered humans)
    /// Permissionless — anyone can call it after 365 days have elapsed.
    pub fn mint_annual_inflation(ctx: Context<MintAnnualInflation>) -> Result<()> {
        require!(!ctx.accounts.program_state.emergency_freeze, EarthError::SystemFrozen);

        let state = &ctx.accounts.program_state;
        let clock = Clock::get()?;

        require!(
            clock.unix_timestamp >= state.last_inflation_time
                .checked_add(ONE_YEAR_SECONDS).ok_or(EarthError::ArithmeticOverflow)?,
            EarthError::InflationNotDueYet
        );

        let total_inflation = state.total_minted
            .checked_mul(INFLATION_BPS).ok_or(EarthError::ArithmeticOverflow)?
            .checked_div(10_000).ok_or(EarthError::ArithmeticOverflow)?;

        require!(total_inflation > 0, EarthError::InflationAmountZero);

        let half = total_inflation.checked_div(2).ok_or(EarthError::ArithmeticOverflow)?;
        // Give remainder to treasury so no tokens are lost
        let treasury_half = total_inflation.checked_sub(half).ok_or(EarthError::ArithmeticOverflow)?;

        let bump = state.mint_authority_bump;
        let signer_seeds: &[&[&[u8]]] = &[&[MINT_AUTHORITY_SEED, &[bump]]];

        // Mint 1.75% → community treasury
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

        // Mint 1.75% → inflation pool (humans claim their share)
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
        // Per-human share = half / total verified humans (0 if no humans yet)
        state.last_inflation_per_human = if state.total_verified_humans > 0 {
            half.checked_div(state.total_verified_humans).unwrap_or(0)
        } else {
            0
        };

        msg!("Annual inflation: {} to treasury, {} to human pool ({} each).",
            treasury_half, half, state.last_inflation_per_human);
        Ok(())
    }

    /// Each active verified human claims their equal share of the annual inflation pool.
    /// Can only be called once per inflation epoch per human.
    pub fn claim_inflation_share(ctx: Context<ClaimInflationShare>) -> Result<()> {
        require!(!ctx.accounts.program_state.emergency_freeze, EarthError::SystemFrozen);

        let state = &ctx.accounts.program_state;
        require!(state.inflation_epoch > 0, EarthError::InflationAmountZero);

        let human = &mut ctx.accounts.human_registry;
        require!(human.is_registered, EarthError::NotRegistered);
        require!(human.is_active, EarthError::HumanNotActive);
        require!(!human.is_deceased, EarthError::HumanDeceased);
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
    // CLAIM VAULT
    // ========================================================================

    /// Verified human claims their 66,000 EARTH allocation.
    pub fn claim_vault(ctx: Context<ClaimVault>) -> Result<()> {
        require!(!ctx.accounts.program_state.emergency_freeze, EarthError::SystemFrozen);

        let vault = &mut ctx.accounts.vault_state;
        require!(vault.is_initialized, EarthError::VaultNotInitialized);
        require!(!vault.is_claimed, EarthError::VaultAlreadyClaimed);
        require_keys_eq!(
            ctx.accounts.beneficiary.key(),
            vault.beneficiary,
            EarthError::UnauthorizedBeneficiary
        );

        let claimer_registry = &ctx.accounts.beneficiary_human_registry;
        require!(claimer_registry.is_registered, EarthError::ClaimerNotHuman);
        require!(claimer_registry.is_active, EarthError::ClaimerNotActive);
        require!(!claimer_registry.is_deceased, EarthError::HumanDeceased);

        if vault.is_minor {
            require!(
                Clock::get()?.unix_timestamp >= vault.unlock_timestamp,
                EarthError::VaultTimeLocked
            );
        }

        let birth_event_id = vault.birth_event_id;
        let vault_bump     = vault.vault_bump;
        let claim_amount   = vault.amount;
        let beneficiary    = vault.beneficiary;
        vault.is_claimed   = true;
        drop(vault); // release mutable borrow before using ctx.accounts.vault_state below

        let signer: &[&[&[u8]]] = &[&[VAULT_SEED, &birth_event_id, &[vault_bump]]];

        token_2022::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from:      ctx.accounts.vault_token_account.to_account_info(),
                    to:        ctx.accounts.beneficiary_token_account.to_account_info(),
                    authority: ctx.accounts.vault_state.to_account_info(),
                },
                signer,
            ),
            claim_amount,
        )?;

        msg!("Vault claimed: {} EARTH to {}", claim_amount, beneficiary);
        Ok(())
    }

    // ========================================================================
    // HUMAN-ONLY TRANSFER
    // ========================================================================

    /// Transfers EARTH between two verified human wallets only.
    pub fn transfer_with_human_check(
        ctx: Context<TransferWithHumanCheck>,
        amount: u64,
    ) -> Result<()> {
        require!(!ctx.accounts.program_state.emergency_freeze, EarthError::SystemFrozen);

        token_2022::transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from:      ctx.accounts.sender_token_account.to_account_info(),
                    to:        ctx.accounts.recipient_token_account.to_account_info(),
                    authority: ctx.accounts.sender.to_account_info(),
                },
            ),
            amount,
        )?;

        msg!("Transfer: {} EARTH between verified humans.", amount);
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
        require!(!state.emergency_freeze, EarthError::SystemFrozen);
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

        msg!("Proposal created. Eligible voters: {}", proposal.total_eligible_voters);
        Ok(())
    }

    pub fn cast_vote(ctx: Context<CastVote>, vote_choice: bool) -> Result<()> {
        require!(!ctx.accounts.program_state.emergency_freeze, EarthError::SystemFrozen);
        require!(ctx.accounts.voter_human_registry.is_registered, EarthError::VoterNotHuman);
        require!(ctx.accounts.voter_human_registry.is_active, EarthError::VoterNotActive);
        require!(!ctx.accounts.voter_human_registry.is_deceased, EarthError::HumanDeceased);
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
        require!(!ctx.accounts.program_state.emergency_freeze, EarthError::SystemFrozen);

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
    // EMERGENCY KILL SWITCH — NO TIME DELAY
    // ========================================================================

    /// Freezes all contract operations immediately.
    /// Can be triggered by primary OR backup admin.
    pub fn emergency_freeze(ctx: Context<EmergencyAction>, reason: [u8; 64]) -> Result<()> {
        let state = &mut ctx.accounts.program_state;
        state.emergency_freeze = true;
        state.freeze_reason    = reason;
        state.freeze_timestamp = Clock::get()?.unix_timestamp;
        msg!("EMERGENCY FREEZE ACTIVATED.");
        Ok(())
    }

    /// Lifts the emergency freeze.
    /// Requires: admin or backup admin signature + a passed governance vote.
    /// No time delay — governance vote is the only gate.
    pub fn emergency_unfreeze(ctx: Context<EmergencyUnfreeze>) -> Result<()> {
        let state = &ctx.accounts.program_state;
        require!(state.emergency_freeze, EarthError::SystemNotFrozen);
        require!(ctx.accounts.unfreeze_proposal.is_executed, EarthError::UnfreezeProposalNotExecuted);
        require!(ctx.accounts.unfreeze_proposal.is_passed, EarthError::UnfreezeProposalNotPassed);
        require!(
            ctx.accounts.unfreeze_proposal.proposal_type == ProposalType::UnfreezeSystem,
            EarthError::WrongProposalType
        );

        let state = &mut ctx.accounts.program_state;
        state.emergency_freeze = false;
        state.freeze_reason    = [0u8; 64];
        state.freeze_timestamp = 0;
        msg!("System unfrozen after governance approval.");
        Ok(())
    }

    // ========================================================================
    // ORACLE UPDATE
    // ========================================================================

    pub fn update_oracle(ctx: Context<AdminOnly>, new_oracle: Pubkey) -> Result<()> {
        require!(!ctx.accounts.program_state.emergency_freeze, EarthError::SystemFrozen);
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
pub struct SetHeir<'info> {
    #[account(mut)]
    pub human: Signer<'info>,

    #[account(
        mut,
        seeds = [HUMAN_REGISTRY_SEED, human.key().as_ref()],
        bump,
        constraint = human_registry.wallet == human.key() @ EarthError::SenderWalletMismatch,
    )]
    pub human_registry: Account<'info, HumanRegistry>,

    /// The proposed heir's registry entry (pass any account if clearing heir).
    pub heir_registry: Account<'info, HumanRegistry>,

    #[account(seeds = [PROGRAM_STATE_SEED], bump, constraint = program_state.is_initialized @ EarthError::NotInitialized)]
    pub program_state: Account<'info, ProgramState>,
}

#[derive(Accounts)]
#[instruction(_birth_event_id: [u8; 32])]
pub struct DeclareDeceased<'info> {
    #[account(mut)]
    pub oracle_signer: Signer<'info>,

    /// CHECK: The deceased person's wallet.
    pub deceased_wallet: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [HUMAN_REGISTRY_SEED, deceased_wallet.key().as_ref()],
        bump,
    )]
    pub human_registry: Account<'info, HumanRegistry>,

    #[account(
        mut,
        seeds = [VAULT_SEED, &_birth_event_id],
        bump,
    )]
    pub vault_state: Account<'info, VaultState>,

    #[account(
        mut,
        constraint = vault_token_account.key() == vault_state.vault_token_account @ EarthError::InvalidVaultTokenAccount,
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,

    /// Destination: heir's token account OR treasury (caller passes the correct one).
    #[account(mut)]
    pub destination_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(seeds = [PROGRAM_STATE_SEED], bump, constraint = program_state.is_initialized @ EarthError::NotInitialized)]
    pub program_state: Account<'info, ProgramState>,

    pub token_program: Program<'info, Token2022>,
}

#[derive(Accounts)]
#[instruction(birth_event_id: [u8; 32])]
pub struct MintBirthAllocation<'info> {
    #[account(mut)]
    pub oracle_signer: Signer<'info>,

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
        init,
        payer = oracle_signer,
        space = 8 + VaultState::INIT_SPACE,
        seeds = [VAULT_SEED, &birth_event_id],
        bump,
    )]
    pub vault_state: Account<'info, VaultState>,

    #[account(
        init,
        payer = oracle_signer,
        token::mint = mint,
        token::authority = vault_state,
        token::token_program = token_program,
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        constraint = treasury_token_account.key() == program_state.treasury_token_account @ EarthError::InvalidTreasuryAccount,
    )]
    pub treasury_token_account: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Program<'info, Token2022>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct MintAnnualInflation<'info> {
    /// CHECK: Permissionless — anyone can trigger.
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
pub struct ClaimVault<'info> {
    #[account(mut)]
    pub beneficiary: Signer<'info>,

    #[account(
        mut,
        constraint = vault_state.is_initialized @ EarthError::VaultNotInitialized,
        constraint = !vault_state.is_claimed @ EarthError::VaultAlreadyClaimed,
        constraint = vault_state.beneficiary == beneficiary.key() @ EarthError::UnauthorizedBeneficiary,
    )]
    pub vault_state: Account<'info, VaultState>,

    #[account(
        mut,
        constraint = vault_token_account.key() == vault_state.vault_token_account @ EarthError::InvalidVaultTokenAccount,
    )]
    pub vault_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub beneficiary_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(
        constraint = beneficiary_human_registry.is_registered @ EarthError::ClaimerNotHuman,
        constraint = beneficiary_human_registry.wallet == beneficiary.key() @ EarthError::ClaimerWalletMismatch,
        seeds = [HUMAN_REGISTRY_SEED, beneficiary.key().as_ref()],
        bump,
    )]
    pub beneficiary_human_registry: Account<'info, HumanRegistry>,

    #[account(seeds = [PROGRAM_STATE_SEED], bump, constraint = program_state.is_initialized @ EarthError::NotInitialized)]
    pub program_state: Account<'info, ProgramState>,

    pub token_program: Program<'info, Token2022>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct TransferWithHumanCheck<'info> {
    #[account(mut)]
    pub sender: Signer<'info>,

    /// CHECK: Recipient wallet.
    pub recipient_wallet: UncheckedAccount<'info>,

    #[account(
        constraint = sender_human_registry.is_registered @ EarthError::SenderNotHuman,
        constraint = sender_human_registry.is_active @ EarthError::HumanNotActive,
        constraint = !sender_human_registry.is_deceased @ EarthError::HumanDeceased,
        constraint = sender_human_registry.wallet == sender.key() @ EarthError::SenderWalletMismatch,
        seeds = [HUMAN_REGISTRY_SEED, sender.key().as_ref()],
        bump,
    )]
    pub sender_human_registry: Account<'info, HumanRegistry>,

    #[account(
        constraint = recipient_human_registry.is_registered @ EarthError::RecipientNotHuman,
        constraint = recipient_human_registry.is_active @ EarthError::HumanNotActive,
        constraint = !recipient_human_registry.is_deceased @ EarthError::HumanDeceased,
        constraint = recipient_human_registry.wallet == recipient_wallet.key() @ EarthError::RecipientWalletMismatch,
        seeds = [HUMAN_REGISTRY_SEED, recipient_wallet.key().as_ref()],
        bump,
    )]
    pub recipient_human_registry: Account<'info, HumanRegistry>,

    #[account(mut)]
    pub sender_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub recipient_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(seeds = [PROGRAM_STATE_SEED], bump, constraint = program_state.is_initialized @ EarthError::NotInitialized)]
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

#[derive(Accounts)]
pub struct EmergencyAction<'info> {
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
pub struct EmergencyUnfreeze<'info> {
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

    pub unfreeze_proposal: Account<'info, Proposal>,
}

// ============================================================================
// STATE ACCOUNTS
// ============================================================================

#[account]
#[derive(InitSpace)]
pub struct ProgramState {
    pub admin_authority:              Pubkey,
    pub backup_authority:             Pubkey,  // Backup admin — in case primary wallet is lost
    pub mint:                         Pubkey,
    pub mint_authority_bump:          u8,
    pub treasury_token_account:       Pubkey,
    pub inflation_pool_token_account: Pubkey,  // 1.75% annual pool claimable by all humans
    pub oracle_data_account:          Pubkey,
    pub total_minted:                 u64,
    pub total_birth_events:           u64,
    pub total_verified_humans:        u64,
    pub total_proposals:              u64,
    pub is_initialized:               bool,
    pub emergency_freeze:             bool,
    pub freeze_reason:                [u8; 64],
    pub freeze_timestamp:             i64,
    pub last_inflation_time:          i64,
    pub inflation_epoch:              u64,     // Increments each year inflation is minted
    pub last_inflation_per_human:     u64,     // Tokens each human can claim this epoch
}

#[account]
#[derive(InitSpace)]
pub struct HumanRegistry {
    pub is_registered:               bool,
    pub iris_hash:                   [u8; 32],
    pub wallet:                      Pubkey,
    pub registration_timestamp:      i64,
    pub is_active:                   bool,
    pub has_voted_count:             u64,
    pub heir:                        Pubkey,  // Wallet to receive unclaimed vault on death
    pub is_deceased:                 bool,
    pub last_inflation_epoch_claimed: u64,   // Last epoch this human claimed inflation share
}

#[account]
#[derive(InitSpace)]
pub struct VaultState {
    pub is_initialized:      bool,
    pub birth_event_id:      [u8; 32],
    pub beneficiary:         Pubkey,
    pub is_minor:            bool,
    pub birth_timestamp:     i64,
    pub unlock_timestamp:    i64,
    pub amount:              u64,
    pub is_claimed:          bool,
    pub vault_token_account: Pubkey,
    pub vault_bump:          u8,
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
    AllocationRelease,
    OracleUpdate,
    EmergencyFreeze,
    UnfreezeSystem,
    InfrastructureDeployment,
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
    #[msg("SYSTEM FROZEN: all operations halted.")]
    SystemFrozen,
    #[msg("System is not currently frozen.")]
    SystemNotFrozen,
    #[msg("Unfreeze proposal has not been executed.")]
    UnfreezeProposalNotExecuted,
    #[msg("Unfreeze proposal did not pass.")]
    UnfreezeProposalNotPassed,
    #[msg("Wrong proposal type for this operation.")]
    WrongProposalType,
    #[msg("Birth event already processed.")]
    BirthEventAlreadyProcessed,
    #[msg("Arithmetic overflow.")]
    ArithmeticOverflow,
    #[msg("Vault not initialized.")]
    VaultNotInitialized,
    #[msg("Vault already claimed.")]
    VaultAlreadyClaimed,
    #[msg("Unauthorized beneficiary.")]
    UnauthorizedBeneficiary,
    #[msg("Vault is time-locked until beneficiary turns 18.")]
    VaultTimeLocked,
    #[msg("Invalid mint account.")]
    InvalidMint,
    #[msg("Invalid vault token account.")]
    InvalidVaultTokenAccount,
    #[msg("Invalid treasury account.")]
    InvalidTreasuryAccount,
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
    #[msg("Sender is not a verified human.")]
    SenderNotHuman,
    #[msg("Sender wallet mismatch.")]
    SenderWalletMismatch,
    #[msg("Recipient is not a verified human.")]
    RecipientNotHuman,
    #[msg("Recipient wallet mismatch.")]
    RecipientWalletMismatch,
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
    #[msg("Human is deceased.")]
    HumanDeceased,
    #[msg("Heir is not a verified human.")]
    HeirNotHuman,
    #[msg("Heir wallet mismatch.")]
    HeirWalletMismatch,
    #[msg("Inflation share already claimed for this epoch.")]
    InflationAlreadyClaimed,
    #[msg("Invalid inflation pool account.")]
    InvalidInflationPoolAccount,
}
