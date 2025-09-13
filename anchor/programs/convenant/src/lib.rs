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

/// Lamports pre-funded into the covenant PDA so it can pay the Magic Action
/// execution fee.
///
/// The covenant PDA doubles as the escrow authority for the post-commit action
/// (`CallHandler::escrow_authority`), and the magic program draws that action's
/// fee from the authority when the action runs on the base layer. Without a
/// reserve the commit succeeds but the action fails to pay and never executes,
/// leaving the covenant stuck on the rollup with its funds escrowed.
const ACTION_FEE_RESERVE: u64 = 500_000_000;

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
        // Provisional: only the creator's lock is in the vault until the
        // counterparty joins, at which point this becomes the real total.
        covenant.escrow_total = creator_amount;
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

        // Pre-fund the covenant PDA with (a) rent for the ephemeral permission
        // account that `init_permission` deposits on the ER after delegation,
        // and (b) the reserve that pays the settlement action's fee. The
        // delegated covenant PDA's lamports are the ones spendable on the ER
        // and on the base-layer action, so both must land here at create time.
        let prefund = ephemeral_rollups_sdk::ephemeral_accounts::rent(
            EphemeralPermission::size_of(2) as u32,
        )
        .saturating_add(ACTION_FEE_RESERVE);
        transfer(
            CpiContext::new(
                anchor_lang::system_program::ID,
                Transfer {
                    from: ctx.accounts.creator.to_account_info(),
                    to: ctx.accounts.covenant.to_account_info(),
                },
            ),
            prefund,
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
        // Both parties' locks are now in the vault. Recorded so the ER-side
        // negotiation can bound a re-price without reading the vault.
        covenant.escrow_total = covenant
            .creator_amount
            .saturating_add(counterparty_amount);
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

    /// A party updates their committed offer during negotiation.
    pub fn update_offer(ctx: Context<Negotiate>, new_amount: u64) -> Result<()> {
        let covenant = &mut ctx.accounts.covenant;
        require!(
            covenant.status == CovenantStatus::ActiveOnEr,
            ConvenantError::InvalidStatus
        );
        require!(
            Clock::get()?.unix_timestamp < covenant.deadline,
            ConvenantError::DeadlineInPast
        );
        require!(new_amount > 0, ConvenantError::ZeroAmount);

        // A party may only re-price its own offer, and only before accepting.
        let is_creator = ctx.accounts.payer.key() == covenant.creator;
        if is_creator {
            require!(!covenant.creator_accepted, ConvenantError::FrozenAfterAccept);
        } else if Some(ctx.accounts.payer.key()) == covenant.counterparty {
            require!(
                !covenant.counterparty_accepted,
                ConvenantError::FrozenAfterAccept
            );
        } else {
            return err!(ConvenantError::UnauthorizedSigner);
        }

        // Solvency bound. Negotiation runs on the ER, where the vault PDA is
        // not delegated and therefore cannot be read, so the bound comes from
        // `escrow_total` — the sum both parties actually locked. The two offers
        // together must stay within it, otherwise `swap_on_l1` would try to
        // release more than the vault holds and settlement would break.
        let combined = if is_creator {
            new_amount.saturating_add(covenant.counterparty_amount)
        } else {
            covenant.creator_amount.saturating_add(new_amount)
        };
        require!(
            combined <= covenant.escrow_total,
            ConvenantError::InsufficientFunds
        );

        // Validate-then-write: nothing is mutated until every check has passed.
        if is_creator {
            covenant.creator_amount = new_amount;
        } else {
            covenant.counterparty_amount = new_amount;
        }

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

    /// Once both parties accept, commit the covenant state back to base layer
    /// and schedule a Magic Action that atomically swaps the vault SOL on the
    /// base layer. Runs on the ER; the commit + swap land together on L1.
    pub fn commit_and_swap(ctx: Context<CommitAndSwap>) -> Result<()> {
        let covenant = &ctx.accounts.covenant;

        require!(
            covenant.status == CovenantStatus::ActiveOnEr,
            ConvenantError::InvalidStatus
        );
        require!(covenant.both_accepted(), ConvenantError::NotBothAccepted);
        require!(
            Clock::get()?.unix_timestamp < covenant.deadline,
            ConvenantError::DeadlineInPast
        );

        let action_data = anchor_lang::InstructionData::data(&crate::instruction::SwapOnL1 {});
        let action_args = ActionArgs::new(action_data);
        let action_accounts = vec![
            ShortAccountMeta {
                pubkey: crate::ID,
                is_writable: false,
            },
            ShortAccountMeta {
                pubkey: ctx.accounts.covenant.key(),
                is_writable: true,
            },
            ShortAccountMeta {
                pubkey: ctx.accounts.vault.key(),
                is_writable: true,
            },
            ShortAccountMeta {
                pubkey: ctx.accounts.creator.key(),
                is_writable: true,
            },
            ShortAccountMeta {
                pubkey: ctx.accounts.counterparty.key(),
                is_writable: true,
            },
            ShortAccountMeta {
                pubkey: ctx.accounts.creator_vault_ata.key(),
                is_writable: true,
            },
            ShortAccountMeta {
                pubkey: ctx.accounts.counterparty_vault_ata.key(),
                is_writable: true,
            },
            ShortAccountMeta {
                pubkey: ctx.accounts.creator_payout_ata.key(),
                is_writable: true,
            },
            ShortAccountMeta {
                pubkey: ctx.accounts.counterparty_payout_ata.key(),
                is_writable: true,
            },
            ShortAccountMeta {
                pubkey: ctx.accounts.creator_mint_acct.key(),
                is_writable: false,
            },
            ShortAccountMeta {
                pubkey: ctx.accounts.counterparty_mint_acct.key(),
                is_writable: false,
            },
            ShortAccountMeta {
                pubkey: ctx.accounts.token_program.key(),
                is_writable: false,
            },
            ShortAccountMeta {
                pubkey: ctx.accounts.system_program.key(),
                is_writable: false,
            },
        ];
        // The magic program requires the action's `escrow_authority` — the
        // covenant PDA — to authorize the scheduled bundle. PDA signing via
        // `invoke_signed` only takes effect for accounts whose CPI meta has
        // `is_signer = true`, and the SDK derives that straight from
        // `AccountInfo::is_signer`. A PDA that is not itself a transaction
        // signer carries `is_signer == false`, so the signature would be
        // silently dropped and the runtime rejects the CPI with
        // `MissingRequiredSignature`. Promote a copy of the account info; the
        // genuine authority comes from the seeds handed to `invoke_signed`.
        let mut escrow_authority_info = ctx.accounts.covenant.to_account_info();
        escrow_authority_info.is_signer = true;

        let action = CallHandler {
            destination_program: crate::ID,
            accounts: action_accounts,
            args: action_args,
            // Signer that authorizes the action and pays its fees; verified in
            // swap_on_l1.
            escrow_authority: escrow_authority_info.clone(),
            compute_units: 200_000,
        };
        let covenant_seeds: &[&[u8]] = &[
            COVENANT_SEED,
            ctx.accounts.covenant.creator.as_ref(),
            &ctx.accounts.covenant.nonce.to_le_bytes(),
            &[ctx.bumps.covenant],
        ];
        MagicIntentBundleBuilder::new(
            ctx.accounts.payer.to_account_info(),
            ctx.accounts.magic_context.to_account_info(),
            ctx.accounts.magic_program.to_account_info(),
        )
        // Both the commit list and the action's escrow authority need the
        // promoted copy. Passing the unmodified AccountInfo here makes the
        // runtime reject the CPI with `MissingRequiredSignature`, because the
        // covenant PDA is never a transaction signer and so arrives with
        // `is_signer == false`.
        //
        // This must be `commit_and_undelegate`, not a plain commit. A plain
        // commit writes the rollup state back but leaves the account owned by
        // the delegation program, so `swap_on_l1` can never validate the
        // covenant (`Account<'info, Covenant>` requires program ownership) and
        // the settlement silently never lands. Undelegating returns ownership
        // to this program first.
        //
        // The action is therefore a *post-undelegate* action: it runs once the
        // covenant is program-owned and writable again.
        .commit_and_undelegate(&[escrow_authority_info])
        .add_post_undelegate_actions([action])
        .build_and_invoke_signed(&[covenant_seeds])?;

        Ok(())
    }

    // -----------------------------------------------------------------
    // Magic Action target — runs on base layer atomically with the commit
    // -----------------------------------------------------------------

    /// Executes the escrow: moves each party's full locked SOL back out of the
    /// vault to the opposite party.
    ///
    /// Authorisation rests on three conditions, all enforced in the Accounts
    /// context or below:
    ///   1. `escrow_auth == covenant` — only a Magic-Action dispatch supplies
    ///      the covenant as the escrow authority.
    ///   2. `creator` / `counterparty` are pinned to the addresses recorded on
    ///      the covenant — value can never be routed to a caller-chosen
    ///      account. This is the property that actually prevents vault theft.
    ///   3. `both_accepted` — the parties agreed to these exact terms.
    pub fn swap_on_l1(ctx: Context<SwapOnL1>) -> Result<()> {
        let covenant = &ctx.accounts.covenant;

        require!(
            ctx.accounts.escrow_auth.key() == covenant.key(),
            ConvenantError::EscrowAuthorityMismatch
        );
        require!(
            covenant.status == CovenantStatus::ActiveOnEr,
            ConvenantError::InvalidStatus
        );
        require!(covenant.both_accepted(), ConvenantError::NotBothAccepted);
        require!(
            Clock::get()?.unix_timestamp < covenant.deadline,
            ConvenantError::DeadlineInPast
        );

        let covenant_key = covenant.key();

        // creator's asset -> counterparty
        release_asset(
            &covenant_key,
            &covenant.creator_asset,
            &covenant.creator_mint,
            covenant.creator_amount,
            &ctx.accounts.vault.to_account_info(),
            &ctx.accounts.counterparty,
            &ctx.accounts.creator_vault_ata,
            &ctx.accounts.counterparty_payout_ata,
            &ctx.accounts.creator_mint_acct,
            &ctx.accounts.token_program.to_account_info(),
            covenant.vault_bump,
        )?;

        // counterparty's asset -> creator
        release_asset(
            &covenant_key,
            &covenant.counterparty_asset,
            &covenant.counterparty_mint,
            covenant.counterparty_amount,
            &ctx.accounts.vault.to_account_info(),
            &ctx.accounts.creator,
            &ctx.accounts.counterparty_vault_ata,
            &ctx.accounts.creator_payout_ata,
            &ctx.accounts.counterparty_mint_acct,
            &ctx.accounts.token_program.to_account_info(),
            covenant.vault_bump,
        )?;

        let covenant = &mut ctx.accounts.covenant;
        covenant.status = CovenantStatus::SettledOnL1;
        covenant.settled_at = Some(Clock::get()?.unix_timestamp);

        Ok(())
    }

    // -----------------------------------------------------------------
    // Expiry refund (base layer)
    // -----------------------------------------------------------------

    /// After the deadline with no settlement, either party refunds the vault.
    pub fn expire_and_refund(ctx: Context<ExpireAndRefund>) -> Result<()> {
        let covenant = &ctx.accounts.covenant;

        require!(
            covenant.status == CovenantStatus::CreatedOnL1
                || covenant.status == CovenantStatus::ActiveOnEr
                || covenant.status == CovenantStatus::Delegating,
            ConvenantError::InvalidStatus
        );
        require!(
            Clock::get()?.unix_timestamp >= covenant.deadline,
            ConvenantError::DeadlineNotPassed
        );
        require!(
            ctx.accounts.signer.key() == covenant.creator
                || Some(ctx.accounts.signer.key()) == covenant.counterparty,
            ConvenantError::UnauthorizedSigner
        );

        let covenant = &ctx.accounts.covenant;
        let covenant_key = covenant.key();

        // Refund creator's locked asset back to the creator.
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
        covenant.status = CovenantStatus::SettledOnL1;
        covenant.settled_at = Some(Clock::get()?.unix_timestamp);

        Ok(())
    }

    /// Create the token account derived for `owner`/`mint` (seeds
    /// `[owner, token_program, mint]`) on the base layer. Replaces the SPL
    /// Associated Token Account program, which is not deployed on the
    /// MagicBlock cluster. Idempotent: no-ops when the account already exists.
    pub fn create_associated_token_account(
        ctx: Context<CreateAssociatedTokenAccount>,
    ) -> Result<()> {
        let token_program = *ctx.accounts.token_program.key;
        require!(token_program == SPL_TOKEN_PROGRAM_ID, ConvenantError::InvalidTokenProgram);

        let ata = &ctx.accounts.ata;
        if ata.lamports() > 0 && ata.owner == &SPL_TOKEN_PROGRAM_ID {
            return Ok(());
        }

        let rent = Rent::get()?.minimum_balance(TOKEN_ACCOUNT_SIZE);
        let bump = ctx.bumps.ata;
        let owner = *ctx.accounts.owner.key;
        let mint = *ctx.accounts.mint.key;
        let signer_seeds: &[&[u8]] = &[
            owner.as_ref(),
            token_program.as_ref(),
            mint.as_ref(),
            &[bump],
        ];

        anchor_lang::system_program::create_account(
            CpiContext::new_with_signer(
                anchor_lang::system_program::ID,
                anchor_lang::system_program::CreateAccount {
                    from: ctx.accounts.payer.to_account_info(),
                    to: ata.to_account_info(),
                },
                &[signer_seeds],
            ),
            rent,
            TOKEN_ACCOUNT_SIZE as u64,
            &token_program,
        )?;

        // SPL InitializeAccount3 (tag 18) — the only variant that works on
        // MagicBlock devnet (tag 0 returns Custom(12)).  Only 3 accounts needed:
        // [account, mint, owner].  Data: [18, ...owner_pubkey].
        let mut data = vec![18u8];
        data.extend_from_slice(&owner.to_bytes());
        let accounts = vec![
            AccountMeta::new(ata.key(), false),
            AccountMeta::new_readonly(mint, false),
            AccountMeta::new_readonly(owner, false),
        ];
        let ix = anchor_lang::solana_program::instruction::Instruction {
            program_id: SPL_TOKEN_PROGRAM_ID,
            accounts,
            data,
        };
        anchor_lang::solana_program::program::invoke(
            &ix,
            &[
                ata.to_account_info(),
                ctx.accounts.mint.to_account_info(),
                ctx.accounts.owner.to_account_info(),
                ctx.accounts.token_program.to_account_info(),
            ],
        )?;

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
pub struct CreateAssociatedTokenAccount<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: derived token account address (seeds owner/token_program/mint)
    #[account(
        mut,
        seeds = [
            owner.key().as_ref(),
            token_program.key().as_ref(),
            mint.key().as_ref(),
        ],
        bump
    )]
    pub ata: SystemAccount<'info>,
    /// CHECK: owner of the new token account (a wallet or this program's vault)
    #[account()]
    pub owner: UncheckedAccount<'info>,
    /// CHECK: SPL mint the new account will hold
    #[account()]
    pub mint: UncheckedAccount<'info>,
    /// CHECK: legacy SPL Token program
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

#[commit]
#[derive(Accounts)]
pub struct CommitAndSwap<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(
        mut,
        seeds = [COVENANT_SEED, covenant.creator.as_ref(), covenant.nonce.to_le_bytes().as_ref()],
        bump
    )]
    pub covenant: Account<'info, Covenant>,
    /// The escrow vault and the token accounts below are **base-layer only**.
    /// They are not delegated to the ER, so passing them as writable accounts
    /// on an ER transaction fails with `InvalidWritableAccount`. This
    /// instruction only needs their addresses — it builds the Magic Action's
    /// account list and schedules it; `swap_on_l1` is what actually touches
    /// them, and that runs on the base layer where they are writable.
    #[account(seeds = [VAULT_SEED, covenant.key().as_ref()], bump)]
    pub vault: SystemAccount<'info>,
    /// CHECK: validated in-context — must equal `covenant.creator`. Address is
    /// read here to build the action's account list; the action itself
    /// constrains the payout.
    #[account(address = covenant.creator @ ConvenantError::CreatorMismatch)]
    pub creator: UncheckedAccount<'info>,
    /// CHECK: validated in-context — must equal the joined counterparty.
    #[account(
        address = covenant.counterparty.ok_or(ConvenantError::CounterpartyNotJoined)?
            @ ConvenantError::CounterpartyMismatch
    )]
    pub counterparty: UncheckedAccount<'info>,
    /// CHECK: vault ATA holding the creator's SPL offer (SOL: unused)
    #[account()]
    pub creator_vault_ata: UncheckedAccount<'info>,
    /// CHECK: vault ATA holding the counterparty's SPL offer (SOL: unused)
    #[account()]
    pub counterparty_vault_ata: UncheckedAccount<'info>,
    /// CHECK: counterparty's payout ATA for the creator's SPL offer (SOL: unused)
    #[account()]
    pub creator_payout_ata: UncheckedAccount<'info>,
    /// CHECK: creator's payout ATA for the counterparty's SPL offer (SOL: unused)
    #[account()]
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
pub struct SwapOnL1<'info> {
    #[account(
        mut,
        seeds = [COVENANT_SEED, covenant.creator.as_ref(), covenant.nonce.to_le_bytes().as_ref()],
        bump
    )]
    pub covenant: Account<'info, Covenant>,
    #[account(mut, seeds = [VAULT_SEED, covenant.key().as_ref()], bump)]
    pub vault: SystemAccount<'info>,
    /// CHECK: validated in-context — must equal `covenant.creator`; receives the
    /// counterparty's offer.
    #[account(mut, address = covenant.creator @ ConvenantError::CreatorMismatch)]
    pub creator: UncheckedAccount<'info>,
    /// CHECK: validated in-context — must equal the joined counterparty.
    #[account(
        mut,
        address = covenant.counterparty.ok_or(ConvenantError::CounterpartyNotJoined)?
            @ ConvenantError::CounterpartyMismatch
    )]
    pub counterparty: UncheckedAccount<'info>,
    /// CHECK: validated in-context — must equal the covenant PDA. The Magic
    /// program dispatches a scheduled action with this PDA as the authorising
    /// escrow authority (see `CallHandler::escrow_authority`), so the scheduled
    /// settlement always supplies the covenant key here.
    ///
    /// NOTE: this is deliberately *not* a `signer` constraint. The Magic
    /// program is the party that would co-sign at dispatch, but its exact
    /// dispatch semantics could not be verified offline; requiring a signature
    /// that the dispatcher does not actually provide would make settlement
    /// permanently uncallable. The settlement is instead authorised by
    /// `both_accepted` plus the recipient pinning below, which is what actually
    /// prevents value leaving the vault to an attacker-chosen account.
    #[account(address = covenant.key() @ ConvenantError::EscrowAuthorityMismatch)]
    pub escrow_auth: UncheckedAccount<'info>,
    /// CHECK: vault ATA holding the creator's SPL offer (SOL: unused)
    #[account(mut)]
    pub creator_vault_ata: UncheckedAccount<'info>,
    /// CHECK: vault ATA holding the counterparty's SPL offer (SOL: unused)
    #[account(mut)]
    pub counterparty_vault_ata: UncheckedAccount<'info>,
    /// CHECK: creator's payout ATA for the counterparty's SPL offer (SOL: unused)
    #[account(mut)]
    pub creator_payout_ata: UncheckedAccount<'info>,
    /// CHECK: counterparty's payout ATA for the creator's SPL offer (SOL: unused)
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
pub struct ExpireAndRefund<'info> {
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
    /// CHECK: validated in-context — must equal `covenant.creator`. Refund
    /// recipients are pinned so the vault PDA can never be instructed to pay a
    /// caller-chosen account.
    #[account(mut, address = covenant.creator @ ConvenantError::CreatorMismatch)]
    pub creator: UncheckedAccount<'info>,
    /// CHECK: validated in-context — must equal the joined counterparty.
    #[account(
        mut,
        address = covenant.counterparty.ok_or(ConvenantError::CounterpartyNotJoined)?
            @ ConvenantError::CounterpartyMismatch
    )]
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
    /// CHECK: validated in-context — must equal `covenant.creator`.
    #[account(mut, address = covenant.creator @ ConvenantError::CreatorMismatch)]
    pub creator: UncheckedAccount<'info>,
    /// CHECK: validated in-context — must equal the joined counterparty.
    #[account(
        mut,
        address = covenant.counterparty.ok_or(ConvenantError::CounterpartyNotJoined)?
            @ ConvenantError::CounterpartyMismatch
    )]
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
