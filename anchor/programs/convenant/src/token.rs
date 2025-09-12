use anchor_lang::prelude::*;
use anchor_lang::system_program::{transfer, Transfer};

use crate::errors::ConvenantError;
use crate::state::{AssetType, VAULT_SEED};

/// This program replaces the SPL Associated Token Account program (which is
/// not deployed on the MagicBlock cluster) as the authority that derives and
/// creates token accounts for users and for the vault. Accounts are derived
/// with seeds `[owner, token_program, mint]` under THIS program's id, and are
/// created on-chain by `create_associated_token_account`.
pub const TOKEN_ACCOUNT_SIZE: usize = 165;

pub const RENT_SYSVAR: Pubkey =
    Pubkey::from_str_const("SysvarRent111111111111111111111111111111111");

/// Legacy SPL Token Program id (USDC and standard NFTs use this program).
pub const SPL_TOKEN_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

/// The (parent) vault PDA that escrows SOL lamports and is the owner of the
/// SPL token vault accounts.
pub fn vault_pda(covenant_key: &Pubkey) -> Pubkey {
    let (v, _) =
        Pubkey::find_program_address(&[VAULT_SEED, covenant_key.as_ref()], &crate::ID);
    v
}

/// Token account of `owner` for `mint`: seeds `[owner, token_program, mint]`
/// under this program (replacing the missing SPL ATA program).
pub fn associated_token_address(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    let (ata, _) = Pubkey::find_program_address(
        &[
            owner.as_ref(),
            SPL_TOKEN_PROGRAM_ID.as_ref(),
            mint.as_ref(),
        ],
        &crate::ID,
    );
    ata
}

/// Vault-owned ATA holding a party's SPL/NFT for a given mint. Must be
/// pre-created by the client before locking.
pub fn vault_ata(covenant_key: &Pubkey, mint: &Pubkey) -> Pubkey {
    associated_token_address(&vault_pda(covenant_key), mint)
}

/// Read `decimals` out of an SPL mint account's data.
/// spl-token Mint layout: [mint_authority(Option<Pubkey>)..supply(8)..decimals
/// @44][...].
pub fn mint_decimals(mint_data: &[u8]) -> Result<u8> {
    if mint_data.len() < 45 {
        return Err(ConvenantError::InvalidMint.into());
    }
    Ok(mint_data[44])
}

/// True when this side's asset is native SOL rather than an SPL/NFT.
pub fn is_sol(asset: &AssetType, mint: &Pubkey) -> bool {
    *asset == AssetType::Sol || *mint == anchor_lang::system_program::ID
}

/// Core SPL transfer helper. Calls `transfer_checked` (legacy token program)
/// signed either by `signer_pda` (seeds) or by an outer real signer (seeds
/// empty). `authority` is the account that authorises the transfer.
///
/// The instruction is built manually (tag 12) to avoid depending on
/// `spl-token`, whose `solana-program 2` dependency would install a second
/// `#[global_allocator]` and conflict under SBF.
fn spl_transfer<'info>(
    source: &AccountInfo<'info>,
    mint: &Pubkey,
    destination: &AccountInfo<'info>,
    authority: &AccountInfo<'info>,
    token_program: &AccountInfo<'info>,
    mint_info: &AccountInfo<'info>,
    amount: u64,
    seeds: &[&[&[u8]]],
) -> Result<()> {
    require!(mint_info.key() == *mint, ConvenantError::InvalidMint);
    let decimals = mint_decimals(&mint_info.try_borrow_data()?)?;

    // data = tag(12) || amount(le u64) || decimals(u8)
    let mut data = vec![12u8];
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);

    let accounts = vec![
        AccountMeta::new(*source.key, false),
        AccountMeta::new_readonly(*mint, false),
        AccountMeta::new(*destination.key, false),
        AccountMeta::new_readonly(*authority.key, true),
    ];
    let ix = anchor_lang::solana_program::instruction::Instruction {
        program_id: SPL_TOKEN_PROGRAM_ID,
        accounts,
        data,
    };
    anchor_lang::solana_program::program::invoke_signed(
        &ix,
        &[
            source.clone(),
            mint_info.clone(),
            destination.clone(),
            authority.clone(),
            token_program.clone(),
        ],
        seeds,
    )?;
    Ok(())
}

/// Lock a party's SPL/NFT into the vault's ATA at initialize/join.
/// `party` is a real signer. Requires the vault ATA to already exist (the
/// client creates it on the base layer before locking).
pub fn lock_spl<'info>(
    source_ata: &AccountInfo<'info>,
    vault_token_account: &AccountInfo<'info>,
    mint: &Pubkey,
    mint_info: &AccountInfo<'info>,
    party: &AccountInfo<'info>,
    token_program: &AccountInfo<'info>,
    amount: u64,
) -> Result<()> {
    spl_transfer(
        source_ata,
        mint,
        vault_token_account,
        party,
        token_program,
        mint_info,
        amount,
        &[], // party signs as the real transaction signer
    )
}

/// Move `amount` of one side's locked asset out of the vault to `recipient`.
/// - SOL: system transfer from the vault (lamports already escrowed there).
/// - SPL/NFT: `transfer_checked` from `vault_token_account` (the vault's ATA)
///   to `recipient_ata`. Both are pre-created accounts supplied by the client.
/// `vault_bump` signs as the vault PDA (both SOL and token paths).
#[allow(clippy::too_many_arguments)]
pub fn release_asset<'info>(
    covenant_key: &Pubkey,
    asset: &AssetType,
    mint: &Pubkey,
    amount: u64,
    vault: &AccountInfo<'info>,
    recipient: &AccountInfo<'info>,
    vault_token_account: &AccountInfo<'info>,
    recipient_ata: &AccountInfo<'info>,
    mint_info: &AccountInfo<'info>,
    token_program: &AccountInfo<'info>,
    vault_bump: u8,
) -> Result<()> {
    if is_sol(asset, mint) {
        transfer(
            CpiContext::new_with_signer(
                anchor_lang::system_program::ID,
                Transfer {
                    from: vault.clone(),
                    to: recipient.clone(),
                },
                &[&[VAULT_SEED, covenant_key.as_ref(), &[vault_bump]]],
            ),
            amount,
        )?;
    } else {
        spl_transfer(
            vault_token_account,
            mint,
            recipient_ata,
            vault, // authority = vault PDA, signed via seeds
            token_program,
            mint_info,
            amount,
            &[&[VAULT_SEED, covenant_key.as_ref(), &[vault_bump]]],
        )?;
    }
    Ok(())
}