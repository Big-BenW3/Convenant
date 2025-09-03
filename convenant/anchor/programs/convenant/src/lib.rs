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

    // -----------------------------------------------------------------
    // Private delegation (base layer, PER)
    // -----------------------------------------------------------------

    /// Delegates the covenant PDA to the ER so the parties can negotiate
    /// privately. Delegate-only (no bundled base-layer permission CPI): the
    /// private access-control account is created *on the ER* afterwards via
    /// `init_permission`, which is the canonical PER flow — it avoids the
    /// base-layer ACL round-trip and the extra base-layer account for the
    /// permission. Users of the convenience `delegate_covenant` never pay for
    /// the permission account on L1.
    pub fn delegate_covenant(ctx: Context<DelegateCovenant>) -> Result<()> {
        let data = ctx.accounts.covenant.try_borrow_data()?;
        require!(
            &data[..8] == &Covenant::DISCRIMINATOR[..],
            ConvenantError::CovenantPdaMismatch
        );
        let mut covenant = Covenant::deserialize(&mut &data[8..])?;
        drop(data);

        // The delegate field is an `UncheckedAccount` (canonical PER pattern), so
        // re-assert the PDA seeds manually and derive the exact seed set for the
        // SDK delegation.
        let actual_key = ctx.accounts.covenant.key();
        let (derived_key, _) = Pubkey::find_program_address(
            &[
                COVENANT_SEED,
                covenant.creator.as_ref(),
                covenant.nonce.to_le_bytes().as_ref(),
            ],
            &crate::ID,
        );
        require!(actual_key == derived_key, ConvenantError::CovenantPdaMismatch);

        require!(
            covenant.counterparty_joined,
            ConvenantError::CounterpartyNotJoined
        );

        // Delegate the covenant PDA itself to the (optionally TEE-pinned) ER.
        // The status is serialized into the covenant *before* the delegation
        // dance: the SDK zeroes the PDA, assigns it to the delegation program,
        // and the delegation program restores the data from the buffer it took
        // before zeroing — so the account carries `ActiveOnEr` onto the ER. Any
        // write after delegation would violate the non-owner data rule.
        if ctx.accounts.covenant.owner != &ephemeral_rollups_sdk::id() {
            covenant.status = CovenantStatus::ActiveOnEr;
            let mut data = ctx.accounts.covenant.try_borrow_mut_data()?;
            covenant.serialize(&mut &mut data[8..])?;
            drop(data);

            ctx.accounts.delegate_covenant(
                &ctx.accounts.payer,
                &[
                    COVENANT_SEED,
                    covenant.creator.as_ref(),
                    covenant.nonce.to_le_bytes().as_ref(),
                ],
                DelegateConfig {
                    validator: ctx.accounts.validator.as_ref().map(|v| v.key()),
                    ..Default::default()
                },
            )?;
        }

        Ok(())
    }

    // -----------------------------------------------------------------
    // Private access control (ephemeral rollup, TEE-enforced)
    // -----------------------------------------------------------------

    /// Creates the private permission account for the covenant directly on the
    /// ER. Runs on the ER after `delegate_covenant`; `payer` = the (delegated)
    /// covenant PDA, which signs with its base seeds and spends its mirrored
    /// lamports as the rent sponsor. Idempotent: skips if it already exists.
    /// Both creator and counterparty are granted full authority (PER) so each
    /// can read the other's committed offer through the TEE, while anyone else
    /// is rejected.
    pub fn init_permission(ctx: Context<PermissionContext>) -> Result<()> {
        let covenant = &ctx.accounts.covenant;

        require!(
            covenant.status == CovenantStatus::ActiveOnEr,
            ConvenantError::InvalidStatus
        );
        require!(
            covenant.is_party(&ctx.accounts.payer.key()),
            ConvenantError::UnauthorizedSigner
        );

        if ctx.accounts.permission.lamports() > 0 {
            msg!("Permission already exists, skipping creation");
            return Ok(());
        }

        let nonce_bytes = covenant.nonce.to_le_bytes();
        let signers = [
            COVENANT_SEED,
            covenant.creator.as_ref(),
            nonce_bytes.as_ref(),
            &[covenant.bump],
        ];
        CreateEphemeralPermissionCpi {
            payer: ctx.accounts.covenant.to_account_info(),
            permissioned_account: ctx.accounts.covenant.to_account_info(),
            permission: ctx.accounts.permission.to_account_info(),
            vault: ctx.accounts.ephemeral_vault.to_account_info(),
            magic_program: ctx.accounts.magic_program.to_account_info(),
            permission_program: ctx.accounts.permission_program.to_account_info(),
            args: EphemeralMembersArgs {
                is_private: true,
                members: vec![
                    Member {
                        flags: AUTHORITY,
                        pubkey: covenant.creator,
                    },
                    Member {
                        flags: AUTHORITY,
                        pubkey: covenant.counterparty.expect("joined before permission"),
                    },
                ],
            },
        }
        .invoke_signed(&[&signers])?;

        Ok(())
    }

    // -----------------------------------------------------------------
    // Cancel / close (base layer)
    // -----------------------------------------------------------------

    /// The creator cancels a still-open covenant (never delegated), refunding
    /// every locked party. Terminal: hits `Reverted`.
    pub fn cancel_covenant(ctx: Context<CancelCovenant>) -> Result<()> {
        let covenant = &ctx.accounts.covenant;

        require!(
            ctx.accounts.signer.key() == covenant.creator,
            ConvenantError::UnauthorizedSigner
        );
        require!(
            covenant.status == CovenantStatus::CreatedOnL1,
            ConvenantError::InvalidStatus
        );

        let covenant_key = covenant.key();

        release_asset(
            &covenant_key,
            &covenant.creator_asset,
            &covenant.creator_mint,
            covenant.creator_amount,
            &ctx.accounts.vault.to_account_info(),
            &ctx.accounts.creator,
            &ctx.accounts.creator_vault_ata,
            &ctx.accounts.creator_payout_ata,
            &ctx.accounts.creator_mint_acct,
            &ctx.accounts.token_program.to_account_info(),
            covenant.vault_bump,
        )?;

        if covenant.counterparty_joined {
            release_asset(
                &covenant_key,
                &covenant.counterparty_asset,
                &covenant.counterparty_mint,
                covenant.counterparty_amount,
                &ctx.accounts.vault.to_account_info(),
                &ctx.accounts.counterparty,
                &ctx.accounts.counterparty_vault_ata,
                &ctx.accounts.counterparty_payout_ata,
                &ctx.accounts.counterparty_mint_acct,
                &ctx.accounts.token_program.to_account_info(),
                covenant.vault_bump,
            )?;
        }

        let covenant = &mut ctx.accounts.covenant;
        covenant.status = CovenantStatus::Reverted;
        covenant.settled_at = Some(Clock::get()?.unix_timestamp);

        Ok(())
    }

    /// The creator closes a terminal covenant, reclaiming the covenant PDA's
    /// lamports (account rent + any leftover permission prefund).
    pub fn close_covenant(ctx: Context<CloseCovenant>) -> Result<()> {
        let covenant = &ctx.accounts.covenant;

        require!(
            ctx.accounts.signer.key() == covenant.creator,
            ConvenantError::UnauthorizedSigner
        );
        require!(
            covenant.status == CovenantStatus::Reverted
                || covenant.status == CovenantStatus::SettledOnL1
                || covenant.status == CovenantStatus::Settling,
            ConvenantError::InvalidStatus
        );

        Ok(())
    }

    // -----------------------------------------------------------------
    // Private negotiation (ephemeral rollup)
    // -----------------------------------------------------------------

    /// A party updates their committed SOL offer during negotiation.
    pub fn update_offer(ctx: Context<Negotiate>, new_amount: u64) -> Result<()> {
        let covenant = &mut ctx.accounts.covenant;
        require!(
            covenant.status == CovenantStatus::ActiveOnEr,
            ConvenantError::InvalidStatus
        );

        if ctx.accounts.payer.key() == covenant.creator {
            require!(!covenant.creator_accepted, ConvenantError::FrozenAfterAccept);
            covenant.creator_amount = new_amount;
        } else if Some(ctx.accounts.payer.key()) == covenant.counterparty {
            require!(!covenant.counterparty_accepted, ConvenantError::FrozenAfterAccept);
            covenant.counterparty_amount = new_amount;
        } else {
            return err!(ConvenantError::UnauthorizedSigner);
        }
        require!(new_amount > 0, ConvenantError::ZeroAmount);

        // Reset acceptance flags when terms change.
        covenant.creator_accepted = false;
        covenant.counterparty_accepted = false;
        covenant.terms_hash = covenant.hash_terms();

        Ok(())
    }

    /// A party accepts the current terms. Both must accept to settle.
    pub fn accept(ctx: Context<Negotiate>) -> Result<()> {
        let covenant = &mut ctx.accounts.covenant;
        require!(
            covenant.status == CovenantStatus::ActiveOnEr,
            ConvenantError::InvalidStatus
        );

        if ctx.accounts.payer.key() == covenant.creator {
            covenant.creator_accepted = true;
        } else if Some(ctx.accounts.payer.key()) == covenant.counterparty {
            covenant.counterparty_accepted = true;
        } else {
            return err!(ConvenantError::UnauthorizedSigner);
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

#[delegate]
#[derive(Accounts)]
pub struct DelegateCovenant<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: The covenant PDA to delegate (private terms live here on the ER).
    /// `UncheckedAccount` + `del` — canonical PER pattern; the PDA seeds are
    /// re-asserted in the handler before the SDK delegation runs.
    #[account(mut, del)]
    pub covenant: UncheckedAccount<'info>,
    /// CHECK: Checked by the delegation program (TEE validator to pin the ER).
    pub validator: Option<UncheckedAccount<'info>>,
}

/// Shared context for `init_permission` — runs on the ER, operates on the
/// ephemeral permission account for the covenant.
#[derive(Accounts)]
pub struct PermissionContext<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(
        mut,
        seeds = [COVENANT_SEED, covenant.creator.as_ref(), covenant.nonce.to_le_bytes().as_ref()],
        bump
    )]
    pub covenant: Account<'info, Covenant>,
    /// CHECK: verified by the permission program; seeds match the on-chain layout
    #[account(
        mut,
        seeds = [PERMISSION_SEED, covenant.key().as_ref()],
        bump,
        seeds::program = PERMISSION_PROGRAM_ID
    )]
    pub permission: UncheckedAccount<'info>,
    /// CHECK: Permission Program
    #[account(address = PERMISSION_PROGRAM_ID)]
    pub permission_program: UncheckedAccount<'info>,
    /// CHECK: verified by magic program
    #[account(mut, address = EPHEMERAL_VAULT_ID)]
    pub ephemeral_vault: UncheckedAccount<'info>,
    /// CHECK: Magic Program
    #[account(address = MAGIC_PROGRAM_ID)]
    pub magic_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct Negotiate<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(
        mut,
        seeds = [COVENANT_SEED, covenant.creator.as_ref(), covenant.nonce.to_le_bytes().as_ref()],
        bump
    )]
    pub covenant: Account<'info, Covenant>,
}

#[derive(Accounts)]
pub struct CancelCovenant<'info> {
    #[account(mut)]
    pub signer: Signer<'info>,
    #[account(
        mut,
        seeds = [COVENANT_SEED, covenant.creator.as_ref(), covenant.nonce.to_le_bytes().as_ref()],
        bump
    )]
    pub covenant: Account<'info, Covenant>,
    #[account(mut, seeds = [VAULT_SEED, covenant.key().as_ref()], bump)]
    pub vault: SystemAccount<'info>,
    /// CHECK: recipients of the refund
    #[account(mut)]
    pub creator: UncheckedAccount<'info>,
    /// CHECK: recipients of the refund
    #[account(mut)]
    pub counterparty: UncheckedAccount<'info>,
    /// CHECK: vault ATA holding the creator's SPL offer (SOL: unused)
    #[account(mut)]
    pub creator_vault_ata: UncheckedAccount<'info>,
    /// CHECK: vault ATA holding the counterparty's SPL offer (SOL: unused)
    #[account(mut)]
    pub counterparty_vault_ata: UncheckedAccount<'info>,
    /// CHECK: creator's payout ATA for the creator's SPL offer (SOL: unused)
    #[account(mut)]
    pub creator_payout_ata: UncheckedAccount<'info>,
    /// CHECK: counterparty's payout ATA for the counterparty's SPL offer (SOL: unused)
    #[account(mut)]
    pub counterparty_payout_ata: UncheckedAccount<'info>,
    /// CHECK: mint account for the creator's SPL offer (decimals); SOL: dummy
    #[account()]
    pub creator_mint_acct: UncheckedAccount<'info>,
    /// CHECK: mint account for the counterparty's SPL offer (decimals)
    #[account()]
    pub counterparty_mint_acct: UncheckedAccount<'info>,
    /// CHECK: legacy SPL Token program
    #[account()]
    pub token_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct CloseCovenant<'info> {
    #[account(mut)]
    pub signer: Signer<'info>,
    #[account(
        mut,
        close = signer,
        seeds = [COVENANT_SEED, covenant.creator.as_ref(), covenant.nonce.to_le_bytes().as_ref()],
        bump
    )]
    pub covenant: Account<'info, Covenant>,
    pub system_program: Program<'info, System>,
}
