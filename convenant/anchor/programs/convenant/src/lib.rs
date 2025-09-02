use anchor_lang::prelude::*;
use anchor_lang::system_program::{transfer, Transfer};

pub mod errors;
pub mod state;
#[cfg(test)]
pub mod tests;
pub mod token;

use errors::ConvenantError;
use state::*;
use token::*;

// MagicBlock Ephemeral Rollups SDK
use ephemeral_rollups_sdk::anchor::{action, commit, delegate, ephemeral};
use ephemeral_rollups_sdk::ephem::{CallHandler, MagicIntentBundleBuilder};
use ephemeral_rollups_sdk::{ActionArgs, ShortAccountMeta};

use ephemeral_rollups_sdk::access_control::instructions::CreateEphemeralPermissionCpi;
use ephemeral_rollups_sdk::access_control::structs::{
    EphemeralMembersArgs, EphemeralPermission, Member, PERMISSION_SEED,
};
use ephemeral_rollups_sdk::consts::{EPHEMERAL_VAULT_ID, MAGIC_PROGRAM_ID, PERMISSION_PROGRAM_ID};
use ephemeral_rollups_sdk::cpi::DelegateConfig;

declare_id!("2uY6gVNHe62XZXByt796pbwye1p7eTu2Nftv7uwC5CtQ");

/// Authority flag grants both members full read access to the private terms
/// (PER). Both the creator and the counterparty are authorities so they each
/// see the other's committed offer privately, while third parties cannot.
const AUTHORITY: u8 = 1 << 0;

#[ephemeral]
#[program]
pub mod convenant {
    use super::*;

    // -----------------------------------------------------------------
    // Base-layer lifecycle (before delegation)
    // -----------------------------------------------------------------

    /// Creator locks their offer and creates the covenant. `counterparty` is
    /// optional: `Some` pins the covenant to a specific counterparty
    /// (invite-only), `None` makes it openly joinable by the first party who
    /// locks a matching offer before the deadline.
    pub fn initialize_covenant(
        ctx: Context<InitializeCovenant>,
        counterparty: Option<Pubkey>,
        creator_asset: AssetType,
        creator_mint: Pubkey,
        creator_amount: u64,
        deadline: i64,
        nonce: u64,
    ) -> Result<()> {
        let covenant = &mut ctx.accounts.covenant;
        let clock = Clock::get()?;

        require!(deadline > clock.unix_timestamp, ConvenantError::DeadlineInPast);
        require!(creator_amount > 0, ConvenantError::ZeroAmount);
        if let Some(counterparty) = counterparty {
            require!(counterparty != ctx.accounts.creator.key(), ConvenantError::SameParty);
            require!(
                counterparty != anchor_lang::system_program::ID,
                ConvenantError::InvalidCounterparty
            );
        }

        covenant.version = 1;
        covenant.creator = ctx.accounts.creator.key();
        covenant.counterparty = counterparty;
        covenant.counterparty_joined = false;
        covenant.creator_amount = creator_amount;
        covenant.creator_asset = creator_asset;
        covenant.creator_mint = creator_mint;
        covenant.counterparty_amount = 0;
        covenant.counterparty_asset = AssetType::Sol;
        covenant.counterparty_mint = anchor_lang::system_program::ID;
        covenant.creator_accepted = false;
        covenant.counterparty_accepted = false;
        covenant.status = CovenantStatus::CreatedOnL1;
        covenant.deadline = deadline;
        covenant.nonce = nonce;
        covenant.bump = ctx.bumps.covenant;
        covenant.vault_bump = ctx.bumps.vault;
        covenant.created_at = clock.unix_timestamp;
        covenant.settled_at = None;
        covenant.terms_hash = covenant.hash_terms();

        // Lock creator's asset into the (base-layer) vault.
        if is_sol(&creator_asset, &creator_mint) {
            transfer(
                CpiContext::new(
                    anchor_lang::system_program::ID,
                    Transfer {
                        from: ctx.accounts.creator.to_account_info(),
                        to: ctx.accounts.vault.to_account_info(),
                    },
                ),
                creator_amount,
            )?;
        } else {
            lock_spl(
                &ctx.accounts.source_token_account,
                &ctx.accounts.vault_token_account,
                &creator_mint,
                &ctx.accounts.mint_account,
                &ctx.accounts.creator.to_account_info(),
                &ctx.accounts.token_program.to_account_info(),
                creator_amount,
            )?;
        }

        // Pre-fund the covenant PDA with rent for the ephemeral permission
        // account that `init_permission` deposits on the ER after delegation
        // (the delegated covenant PDA's lamports are the ones spendable there).
        transfer(
            CpiContext::new(
                anchor_lang::system_program::ID,
                Transfer {
                    from: ctx.accounts.creator.to_account_info(),
                    to: ctx.accounts.covenant.to_account_info(),
                },
            ),
            ephemeral_rollups_sdk::ephemeral_accounts::rent(
                EphemeralPermission::size_of(2) as u32,
            ),
        )?;

        Ok(())
    }

    /// A party joins and locks their offer. If the creator pinned a specific
    /// counterparty at creation it must be this signer; otherwise any wallet
    /// (except the creator) that arrives before the deadline can join — the
    /// covenant is set when the first taker locks their assets.
    pub fn join_covenant(
        ctx: Context<JoinCovenant>,
        counterparty_asset: AssetType,
        counterparty_mint: Pubkey,
        counterparty_amount: u64,
    ) -> Result<()> {
        let covenant = &mut ctx.accounts.covenant;
        let signer = ctx.accounts.counterparty.key();

        require!(!covenant.counterparty_joined, ConvenantError::CounterpartyAlreadyJoined);
        require!(signer != covenant.creator, ConvenantError::SameParty);
        require!(
            covenant.status == CovenantStatus::CreatedOnL1,
            ConvenantError::InvalidStatus
        );
        require!(
            Clock::get()?.unix_timestamp < covenant.deadline,
            ConvenantError::DeadlineInPast
        );
        require!(counterparty_amount > 0, ConvenantError::ZeroAmount);

        match covenant.counterparty {
            Some(expected) => require!(signer == expected, ConvenantError::UnauthorizedSigner),
            None => covenant.counterparty = Some(signer),
        }

        covenant.counterparty_amount = counterparty_amount;
        covenant.counterparty_asset = counterparty_asset;
        covenant.counterparty_mint = counterparty_mint;
        covenant.counterparty_joined = true;
        covenant.terms_hash = covenant.hash_terms();

        if is_sol(&counterparty_asset, &counterparty_mint) {
            transfer(
                CpiContext::new(
                    anchor_lang::system_program::ID,
                    Transfer {
                        from: ctx.accounts.counterparty.to_account_info(),
                        to: ctx.accounts.vault.to_account_info(),
                    },
                ),
                counterparty_amount,
            )?;
        } else {
            lock_spl(
                &ctx.accounts.source_token_account,
                &ctx.accounts.vault_token_account,
                &counterparty_mint,
                &ctx.accounts.mint_account,
                &ctx.accounts.counterparty.to_account_info(),
                &ctx.accounts.token_program.to_account_info(),
                counterparty_amount,
            )?;
        }

        Ok(())
    }
}

#[derive(Accounts)]
#[instruction(counterparty: Option<Pubkey>, creator_asset: AssetType, creator_mint: Pubkey, creator_amount: u64, deadline: i64, nonce: u64)]
pub struct InitializeCovenant<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,
    #[account(
        init,
        payer = creator,
        space = 8 + Covenant::INIT_SPACE,
        seeds = [COVENANT_SEED, creator.key().as_ref(), nonce.to_le_bytes().as_ref()],
        bump
    )]
    pub covenant: Account<'info, Covenant>,
    /// CHECK: vault PDA holding escrowed SOL
    #[account(mut, seeds = [VAULT_SEED, covenant.key().as_ref()], bump)]
    pub vault: SystemAccount<'info>,
    /// CHECK: party's own ATA holding the SPL/NFT being locked (SOL: unused)
    #[account(mut)]
    pub source_token_account: UncheckedAccount<'info>,
    /// CHECK: vault's ATA receiving the SPL/NFT (SOL: unused)
    #[account(mut)]
    pub vault_token_account: UncheckedAccount<'info>,
    /// CHECK: mint of the SPL/NFT being locked (SOL: unused)
    #[account()]
    pub mint_account: UncheckedAccount<'info>,
    /// CHECK: legacy SPL Token program (SOL: unused)
    #[account()]
    pub token_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct JoinCovenant<'info> {
    #[account(mut)]
    pub counterparty: Signer<'info>,
    #[account(
        mut,
        seeds = [COVENANT_SEED, covenant.creator.as_ref(), covenant.nonce.to_le_bytes().as_ref()],
        bump
    )]
    pub covenant: Account<'info, Covenant>,
    #[account(mut, seeds = [VAULT_SEED, covenant.key().as_ref()], bump)]
    pub vault: SystemAccount<'info>,
    /// CHECK: party's own ATA holding the SPL/NFT being locked (SOL: unused)
    #[account(mut)]
    pub source_token_account: UncheckedAccount<'info>,
    /// CHECK: vault's ATA receiving the SPL/NFT (SOL: unused)
    #[account(mut)]
    pub vault_token_account: UncheckedAccount<'info>,
    /// CHECK: mint of the SPL/NFT being locked (SOL: unused)
    #[account()]
    pub mint_account: UncheckedAccount<'info>,
    /// CHECK: legacy SPL Token program (SOL: unused)
    #[account()]
    pub token_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}
