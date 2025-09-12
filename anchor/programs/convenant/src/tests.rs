use anchor_lang::system_program;
use litesvm::LiteSVM;
use solana_account::Account;
use solana_sdk::{
    clock::Clock,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::Transaction,
};

use crate::state::AssetType;
use crate::ID as PROGRAM_ID;

const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

/// Legacy SPL Token Program (built-in to LiteSVM via with_default_programs).
const TOKEN_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
/// Associated Token Program.
const ATA_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
/// Rent sysvar.
const RENT_ID: Pubkey = Pubkey::from_str_const("SysvarRent111111111111111111111111111111111");
/// Clock sysvar.
const CLOCK_ID: Pubkey = Pubkey::from_str_const("SysvarC1ock11111111111111111111111111111111");

fn current_ts(svm: &LiteSVM) -> i64 {
    let acct = svm.get_account(&CLOCK_ID).unwrap();
    // Clock: slot(8) epoch_start_ts(8) epoch(8) leader_schedule_epoch(8) unix_ts(8)
    i64::from_le_bytes(acct.data[32..40].try_into().unwrap())
}

const COVENANT_SEED: &[u8] = b"covenant";
const VAULT_SEED: &[u8] = b"vault";

// Instruction discriminators: sha256("global:<name>")[0..8].
const DISC_INITIALIZE_COVENANT: [u8; 8] = [72, 61, 129, 133, 204, 182, 242, 247];
const DISC_JOIN_COVENANT: [u8; 8] = [123, 62, 107, 56, 142, 222, 187, 216];
const DISC_SWAP_ON_L1: [u8; 8] = [207, 160, 90, 203, 40, 73, 204, 51];
const DISC_EXPIRE_AND_REFUND: [u8; 8] = [129, 130, 91, 224, 222, 230, 183, 134];
const DISC_CANCEL_COVENANT: [u8; 8] = [245, 55, 132, 8, 37, 126, 233, 151];
const DISC_CLOSE_COVENANT: [u8; 8] = [117, 43, 73, 49, 177, 109, 215, 195];

fn covenant_pda(creator: &Pubkey, nonce: u64) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[COVENANT_SEED, creator.as_ref(), nonce.to_le_bytes().as_ref()],
        &PROGRAM_ID,
    )
}

fn vault_pda(covenant: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[VAULT_SEED, covenant.as_ref()], &PROGRAM_ID)
}

fn ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    let (a, _) = Pubkey::find_program_address(
        &[owner.as_ref(), TOKEN_PROGRAM_ID.as_ref(), mint.as_ref()],
        &ATA_PROGRAM_ID,
    );
    a
}

fn assert_tx_ok(svm: &mut LiteSVM, payer: &Keypair, ixs: &[Instruction]) {
    let blockhash = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(
        &ixs,
        Some(&payer.pubkey()),
        &[&payer],
        blockhash,
    );
    let res = svm.send_transaction(tx);
    if let Err(e) = &res {
        panic!("transaction failed: {:?}", e);
    }
    res.unwrap();
}

// ---------------------------------------------------------------------
// Token helpers (drive the real spl-token program in litesvm)
// ---------------------------------------------------------------------

fn create_mint(svm: &mut LiteSVM, mint: &Keypair, decimals: u8) {
    // Create a rent-exempt mint account owned by the token program, then
    // initialize it (avoids crafting a system CreateAccount instruction).
    let rent = svm.minimum_balance_for_rent_exemption(82);
    svm.set_account(
        mint.pubkey(),
        Account {
            lamports: rent,
            data: vec![0u8; 82],
            owner: TOKEN_PROGRAM_ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();

    // initialize_mint: tag = decimals + mint_authority + COption<0>
    // The mint account is owned by the token program, so it cannot pay
    // transaction fees; use a fresh system-owned payer instead.
    let mut data = vec![0u8];
    data.push(decimals);
    data.extend_from_slice(&mint.pubkey().to_bytes()); // mint authority = keypair
    data.extend_from_slice(&0u32.to_le_bytes()); // COption None
    let ix = Instruction {
        program_id: TOKEN_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(mint.pubkey(), false),
            AccountMeta::new_readonly(RENT_ID, false), // rent sysvar
        ],
        data,
    };
    let payer = fresh_payer(svm);
    let blockhash = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&payer.pubkey()),
        &[&payer],
        blockhash,
    );
    svm.send_transaction(tx).unwrap();
}

fn create_ata(svm: &mut LiteSVM, payer: &Keypair, owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    let ata_addr = ata(owner, mint);
    let ix = Instruction {
        program_id: ATA_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(payer.pubkey(), true),
            AccountMeta::new(ata_addr, false),
            AccountMeta::new_readonly(*owner, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(system_program::ID, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
        ],
        data: vec![0u8],
    };
    assert_tx_ok(svm, payer, &[ix]);
    ata_addr
}

fn mint_to(
    svm: &mut LiteSVM,
    authority: &Keypair,
    mint: &Pubkey,
    dest: &Pubkey,
    amount: u64,
) {
    // tag 7 = mint_to; data = tag + amount(8)
    let mut data = vec![7u8];
    data.extend_from_slice(&amount.to_le_bytes());
    let ix = Instruction {
        program_id: TOKEN_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*mint, false),
            AccountMeta::new(*dest, false),
            AccountMeta::new_readonly(authority.pubkey(), true),
        ],
        data,
    };
    // The mint authority may be a token-owned mint account; use a fresh
    // system-owned fee payer and let `authority` sign as the mint authority.
    let payer = fresh_payer(svm);
    let blockhash = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&payer.pubkey()),
        &[&payer, authority],
        blockhash,
    );
    svm.send_transaction(tx).unwrap();
}

/// Fresh airdropped fee payer (system-owned, so it can pay fees).
fn fresh_payer(svm: &mut LiteSVM) -> Keypair {
    let kp = Keypair::new();
    svm.airdrop(&kp.pubkey(), 1 * LAMPORTS_PER_SOL).unwrap();
    kp
}

fn token_balance(svm: &LiteSVM, ata_addr: &Pubkey) -> u64 {
    match svm.get_account(ata_addr) {
        Some(acct) => u64::from_le_bytes(acct.data[64..72].try_into().unwrap()),
        None => 0,
    }
}

/// Simulates the ER having committed both parties' acceptance: flips the
/// covenant's `creator_accepted`/`counterparty_accepted` flags and its status
/// to `ActiveOnEr` (the state swap_on_l1 requires, normally produced on the
/// ephemeral rollup). Offsets are the borsh field positions within the Anchor
/// account data (after the 8-byte discriminator).
fn activate_covenant(svm: &mut LiteSVM, covenant: &Pubkey) {
    let mut acct = svm.get_account(covenant).unwrap();
    // 8-byte discriminator + borsh offsets (Option<Pubkey> counterparty):
    // creator_accepted = 8+149, counterparty_accepted = 8+150, status = 8+183
    assert!(acct.data.len() >= 8 + 218, "unexpected covenant data size");
    acct.data[157] = 1; // creator_accepted = true
    acct.data[158] = 1; // counterparty_accepted = true
    acct.data[191] = 2; // CovenantStatus::ActiveOnEr
    svm.set_account(*covenant, acct).unwrap();
}

// ---------------------------------------------------------------------
// Convenant instruction builders
// ---------------------------------------------------------------------

fn initialize_ix(
    creator: &Pubkey,
    counterparty: Option<&Pubkey>,
    creator_asset: AssetType,
    creator_mint: &Pubkey,
    creator_amount: u64,
    deadline: i64,
    nonce: u64,
    covenant: &Pubkey,
    vault: &Pubkey,
    source_token_account: &Pubkey,
    vault_token_account: &Pubkey,
    mint_account: &Pubkey,
) -> Instruction {
    // counterparty(1 tag + 32) + asset_enum(1) + mint(32) + amount(8) + deadline(8) + nonce(8)
    let mut data = DISC_INITIALIZE_COVENANT.to_vec();
    match counterparty {
        Some(pk) => {
            data.push(1);
            data.extend_from_slice(pk.as_ref());
        }
        None => data.push(0),
    }
    data.push(creator_asset as u8);
    data.extend_from_slice(creator_mint.as_ref());
    data.extend_from_slice(&creator_amount.to_le_bytes());
    data.extend_from_slice(&deadline.to_le_bytes());
    data.extend_from_slice(&nonce.to_le_bytes());

    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*creator, true),
            AccountMeta::new(*covenant, false),
            AccountMeta::new(*vault, false),
            AccountMeta::new(*source_token_account, false),
            AccountMeta::new(*vault_token_account, false),
            AccountMeta::new_readonly(*mint_account, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data,
    }
}

fn join_ix(
    counterparty: &Pubkey,
    counterparty_asset: AssetType,
    counterparty_mint: &Pubkey,
    counterparty_amount: u64,
    covenant: &Pubkey,
    vault: &Pubkey,
    source_token_account: &Pubkey,
    vault_token_account: &Pubkey,
    mint_account: &Pubkey,
) -> Instruction {
    let mut data = DISC_JOIN_COVENANT.to_vec();
    data.push(counterparty_asset as u8);
    data.extend_from_slice(counterparty_mint.as_ref());
    data.extend_from_slice(&counterparty_amount.to_le_bytes());

    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*counterparty, true),
            AccountMeta::new(*covenant, false),
            AccountMeta::new(*vault, false),
            AccountMeta::new(*source_token_account, false),
            AccountMeta::new(*vault_token_account, false),
            AccountMeta::new_readonly(*mint_account, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data,
    }
}

fn swap_ix(
    covenant: &Pubkey,
    vault: &Pubkey,
    creator: &Pubkey,
    counterparty: &Pubkey,
    escrow_auth: &Pubkey,
    creator_vault_ata: &Pubkey,
    counterparty_vault_ata: &Pubkey,
    creator_payout_ata: &Pubkey,
    counterparty_payout_ata: &Pubkey,
    creator_mint: &Pubkey,
    counterparty_mint: &Pubkey,
    escrow: &Pubkey,
) -> Instruction {
    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*covenant, false),
            AccountMeta::new(*vault, false),
            AccountMeta::new(*creator, false),
            AccountMeta::new(*counterparty, false),
            AccountMeta::new_readonly(*escrow_auth, false),
            AccountMeta::new(*creator_vault_ata, false),
            AccountMeta::new(*counterparty_vault_ata, false),
            AccountMeta::new(*creator_payout_ata, false),
            AccountMeta::new(*counterparty_payout_ata, false),
            AccountMeta::new_readonly(*creator_mint, false),
            AccountMeta::new_readonly(*counterparty_mint, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(system_program::ID, false),
            AccountMeta::new_readonly(*escrow, false),
        ],
        data: DISC_SWAP_ON_L1.to_vec(),
    }
}

fn expire_ix(
    signer: &Pubkey,
    covenant: &Pubkey,
    vault: &Pubkey,
    creator: &Pubkey,
    counterparty: &Pubkey,
    creator_vault_ata: &Pubkey,
    counterparty_vault_ata: &Pubkey,
    creator_payout_ata: &Pubkey,
    counterparty_payout_ata: &Pubkey,
    creator_mint: &Pubkey,
    counterparty_mint: &Pubkey,
) -> Instruction {
    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*signer, true),
            AccountMeta::new(*covenant, false),
            AccountMeta::new(*vault, false),
            AccountMeta::new(*creator, false),
            AccountMeta::new(*counterparty, false),
            AccountMeta::new(*creator_vault_ata, false),
            AccountMeta::new(*counterparty_vault_ata, false),
            AccountMeta::new(*creator_payout_ata, false),
            AccountMeta::new(*counterparty_payout_ata, false),
            AccountMeta::new_readonly(*creator_mint, false),
            AccountMeta::new_readonly(*counterparty_mint, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data: DISC_EXPIRE_AND_REFUND.to_vec(),
    }
}

fn cancel_ix(
    signer: &Pubkey,
    covenant: &Pubkey,
    vault: &Pubkey,
    creator: &Pubkey,
    counterparty: &Pubkey,
    creator_vault_ata: &Pubkey,
    counterparty_vault_ata: &Pubkey,
    creator_payout_ata: &Pubkey,
    counterparty_payout_ata: &Pubkey,
    creator_mint: &Pubkey,
    counterparty_mint: &Pubkey,
) -> Instruction {
    let mut ix = expire_ix(
        signer,
        covenant,
        vault,
        creator,
        counterparty,
        creator_vault_ata,
        counterparty_vault_ata,
        creator_payout_ata,
        counterparty_payout_ata,
        creator_mint,
        counterparty_mint,
    );
    ix.data = DISC_CANCEL_COVENANT.to_vec();
    ix
}

fn close_ix(signer: &Pubkey, covenant: &Pubkey) -> Instruction {
    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*signer, true),
            AccountMeta::new(*covenant, false),
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data: DISC_CLOSE_COVENANT.to_vec(),
    }
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

fn setup_program() -> LiteSVM {
    let mut svm = LiteSVM::new();
    let program_bytes = include_bytes!("../../../target/deploy/convenant.so");
    svm.add_program(PROGRAM_ID, program_bytes).unwrap();
    svm
}

#[test]
fn test_sol_sol_swap() {
    let mut svm = setup_program();

    let creator = Keypair::new();
    let counterparty = Keypair::new();
    svm.airdrop(&creator.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
    svm.airdrop(&counterparty.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

    let creator_sol = 2 * LAMPORTS_PER_SOL;
    let cp_sol = 1 * LAMPORTS_PER_SOL;

    let (covenant, _) = covenant_pda(&creator.pubkey(), 0);
    let (vault, _) = vault_pda(&covenant);

    let init = initialize_ix(
        &creator.pubkey(),
        Some(&counterparty.pubkey()),
        AssetType::Sol,
        &system_program::ID,
        creator_sol,
        current_ts(&svm) + 1000,
        0,
        &covenant,
        &vault,
        &vault,
        &vault,
        &system_program::ID,
    );
    assert_tx_ok(&mut svm, &creator, &[init]);

    let join = join_ix(
        &counterparty.pubkey(),
        AssetType::Sol,
        &system_program::ID,
        cp_sol,
        &covenant,
        &vault,
        &vault,
        &vault,
        &system_program::ID,
    );
    assert_tx_ok(&mut svm, &counterparty, &[join]);

    let vault_bal_before = svm.get_account(&vault).unwrap().lamports;
    assert_eq!(vault_bal_before, creator_sol + cp_sol);

    let creator_before = svm.get_balance(&creator.pubkey()).unwrap();

    // Simulate ER-side acceptance before settling.
    activate_covenant(&mut svm, &covenant);

    // settle
    let swap = swap_ix(
        &covenant,
        &vault,
        &creator.pubkey(),
        &counterparty.pubkey(),
        &covenant,
        &vault,
        &vault,
        &vault,
        &vault,
        &system_program::ID,
        &system_program::ID,
        &system_program::ID,
    );
    // payer is creator
    let blockhash = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(
        &[swap],
        Some(&creator.pubkey()),
        &[&creator],
        blockhash,
    );
    svm.send_transaction(tx).unwrap();

    // vault empty now
    let vault_account = svm.get_account(&vault);
    assert!(
        vault_account.is_none() || vault_account.unwrap().lamports == 0,
        "vault should be empty after swap"
    );

    // creator received counterparty's 1 SOL
    let creator_after = svm.get_balance(&creator.pubkey()).unwrap();
    assert!(creator_after > creator_before);
    // counterparty received creator's 2 SOL
    let cp_after = svm.get_balance(&counterparty.pubkey()).unwrap();
    assert!(cp_after >= 2 * LAMPORTS_PER_SOL);
}

#[test]
fn test_sol_sol_expire_refund() {
    let mut svm = setup_program();

    let creator = Keypair::new();
    let counterparty = Keypair::new();
    svm.airdrop(&creator.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
    svm.airdrop(&counterparty.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

    let creator_sol = 2 * LAMPORTS_PER_SOL;
    let cp_sol = 1 * LAMPORTS_PER_SOL;

    let (covenant, _) = covenant_pda(&creator.pubkey(), 0);
    let (vault, _) = vault_pda(&covenant);

    let init = initialize_ix(
        &creator.pubkey(),
        Some(&counterparty.pubkey()),
        AssetType::Sol,
        &system_program::ID,
        creator_sol,
        current_ts(&svm) + 1000,
        0,
        &covenant,
        &vault,
        &vault,
        &vault,
        &system_program::ID,
    );
    assert_tx_ok(&mut svm, &creator, &[init]);

    let join = join_ix(
        &counterparty.pubkey(),
        AssetType::Sol,
        &system_program::ID,
        cp_sol,
        &covenant,
        &vault,
        &vault,
        &vault,
        &system_program::ID,
    );
    assert_tx_ok(&mut svm, &counterparty, &[join]);

    // Refund each their own SOL.
    // Warp the clock past the deadline.
    let mut clock = svm.get_sysvar::<Clock>();
    clock.unix_timestamp += 10_000;
    svm.set_sysvar::<Clock>(&clock);

    let expire = expire_ix(
        &creator.pubkey(),
        &covenant,
        &vault,
        &creator.pubkey(),
        &counterparty.pubkey(),
        &vault,
        &vault,
        &vault,
        &vault,
        &system_program::ID,
        &system_program::ID,
    );
    assert_tx_ok(&mut svm, &creator, &[expire]);

    let vault_account = svm.get_account(&vault);
    assert!(
        vault_account.is_none() || vault_account.unwrap().lamports == 0,
        "vault should be empty after refund"
    );
    let cp_after = svm.get_balance(&counterparty.pubkey()).unwrap();
    assert!(cp_after >= cp_sol, "counterparty should recover their SOL");
}
/// Common setup: create a token mint with `supply` minted to `holder`'s ATA.
/// Returns (mint_keypair, holder_ata).
fn setup_token(
    svm: &mut LiteSVM,
    _payer: &Keypair,
    holder: &Pubkey,
    decimals: u8,
    supply: u64,
) -> (Keypair, Pubkey) {
    let mint = Keypair::new();
    create_mint(svm, &mint, decimals);
    // The mint account is now token-owned, so it cannot pay fees; fund the
    // holder ATA with a fresh system-owned payer.
    let payer = fresh_payer(svm);
    let holder_ata = create_ata(svm, &payer, holder, &mint.pubkey());
    if supply > 0 {
        mint_to(svm, &mint, &mint.pubkey(), &holder_ata, supply);
    }
    (mint, holder_ata)
}

#[test]
fn test_usdc_sol_swap() {
    let mut svm = setup_program();

    let creator = Keypair::new();
    let counterparty = Keypair::new();
    svm.airdrop(&creator.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
    svm.airdrop(&counterparty.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

    // Creator holds 5_000 USDC (mint authority = Mint keypair).
    let (usdc, creator_usdc) = setup_token(&mut svm, &creator, &creator.pubkey(), 6, 5_000);
    let cp_sol = 1 * LAMPORTS_PER_SOL;
    let usdc_locked = 1_234u64;

    let (covenant, _) = covenant_pda(&creator.pubkey(), 0);
    let (vault, _) = vault_pda(&covenant);

    // Vault USDC ATA must be pre-created (client responsibility).
    let vault_usdc = create_ata(&mut svm, &creator, &vault, &usdc.pubkey());

    let init = initialize_ix(
        &creator.pubkey(),
        Some(&counterparty.pubkey()),
        AssetType::Token,
        &usdc.pubkey(),
        usdc_locked,
        current_ts(&svm) + 1000,
        0,
        &covenant,
        &vault,
        &creator_usdc,
        &vault_usdc,
        &usdc.pubkey(),
    );
    assert_tx_ok(&mut svm, &creator, &[init]);

    let join = join_ix(
        &counterparty.pubkey(),
        AssetType::Sol,
        &system_program::ID,
        cp_sol,
        &covenant,
        &vault,
        &vault,
        &vault,
        &system_program::ID,
    );
    assert_tx_ok(&mut svm, &counterparty, &[join]);

    // Vault now holds the USDC.
    assert_eq!(token_balance(&svm, &vault_usdc), usdc_locked);

    // Counterparty's USDC ATA (to receive creator's USDC).
    let cp_usdc = create_ata(&mut svm, &counterparty, &counterparty.pubkey(), &usdc.pubkey());

    let creator_before = svm.get_balance(&creator.pubkey()).unwrap();

    // Simulate ER-side acceptance before settling.
    activate_covenant(&mut svm, &covenant);

    let swap = swap_ix(
        &covenant,
        &vault,
        &creator.pubkey(),
        &counterparty.pubkey(),
        &covenant,
        &vault_usdc,       // creator's vault ATA (source of creator's USDC)
        &vault,            // counterparty vault ATA (SOL side, unused but mut)
        &vault,            // creator payout ATA (for cp's SOL, unused but mut)
        &cp_usdc,          // counterparty payout ATA (receives creator's USDC)
        &usdc.pubkey(),    // creator mint acct
        &system_program::ID, // counterparty mint acct (SOL, unused)
        &system_program::ID, // escrow
    );
    assert_tx_ok(&mut svm, &creator, &[swap]);

    // Vault USDC emptied into counterparty's USDC ATA.
    assert_eq!(token_balance(&svm, &vault_usdc), 0);
    assert_eq!(token_balance(&svm, &cp_usdc), usdc_locked);
    // Creator received counterparty's SOL.
    assert!(svm.get_balance(&creator.pubkey()).unwrap() > creator_before);
}

#[test]
fn test_nft_sol_swap() {
    let mut svm = setup_program();

    let creator = Keypair::new();
    let counterparty = Keypair::new();
    svm.airdrop(&creator.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
    svm.airdrop(&counterparty.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

    let (nft, creator_nft_ata) = setup_token(&mut svm, &creator, &creator.pubkey(), 0, 1);
    let cp_sol = 1 * LAMPORTS_PER_SOL;

    let (covenant, _) = covenant_pda(&creator.pubkey(), 0);
    let (vault, _) = vault_pda(&covenant);
    let vault_nft = create_ata(&mut svm, &creator, &vault, &nft.pubkey());

    let init = initialize_ix(
        &creator.pubkey(),
        Some(&counterparty.pubkey()),
        AssetType::Nft,
        &nft.pubkey(),
        1,
        current_ts(&svm) + 1000,
        0,
        &covenant,
        &vault,
        &creator_nft_ata,
        &vault_nft,
        &nft.pubkey(),
    );
    assert_tx_ok(&mut svm, &creator, &[init]);

    let join = join_ix(
        &counterparty.pubkey(),
        AssetType::Sol,
        &system_program::ID,
        cp_sol,
        &covenant,
        &vault,
        &vault,
        &vault,
        &system_program::ID,
    );
    assert_tx_ok(&mut svm, &counterparty, &[join]);

    let cp_nft_ata = create_ata(&mut svm, &counterparty, &counterparty.pubkey(), &nft.pubkey());

    // Simulate ER-side acceptance before settling.
    activate_covenant(&mut svm, &covenant);

    let swap = swap_ix(
        &covenant,
        &vault,
        &creator.pubkey(),
        &counterparty.pubkey(),
        &covenant,
        &vault_nft,
        &vault,
        &vault,
        &cp_nft_ata,
        &nft.pubkey(),
        &system_program::ID,
        &system_program::ID,
    );
    assert_tx_ok(&mut svm, &creator, &[swap]);

    assert_eq!(token_balance(&svm, &vault_nft), 0);
    assert_eq!(token_balance(&svm, &cp_nft_ata), 1);
}

#[test]
fn test_usdc_usdc_swap() {
    let mut svm = setup_program();

    let creator = Keypair::new();
    let counterparty = Keypair::new();
    svm.airdrop(&creator.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
    svm.airdrop(&counterparty.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

    let (usdc, creator_usdc) = setup_token(&mut svm, &creator, &creator.pubkey(), 6, 10_000);
    // Counterparty uses the same mint; mint more to their ATA.
    let cp_usdc_holder = create_ata(&mut svm, &creator, &counterparty.pubkey(), &usdc.pubkey());
    mint_to(&mut svm, &usdc, &usdc.pubkey(), &cp_usdc_holder, 10_000);
    let creator_lock = 1_000u64;
    let cp_lock = 500u64;

    let (covenant, _) = covenant_pda(&creator.pubkey(), 0);
    let (vault, _) = vault_pda(&covenant);
    let vault_usdc = create_ata(&mut svm, &creator, &vault, &usdc.pubkey());

    let init = initialize_ix(
        &creator.pubkey(),
        Some(&counterparty.pubkey()),
        AssetType::Token,
        &usdc.pubkey(),
        creator_lock,
        current_ts(&svm) + 1000,
        0,
        &covenant,
        &vault,
        &creator_usdc,
        &vault_usdc,
        &usdc.pubkey(),
    );
    assert_tx_ok(&mut svm, &creator, &[init]);

    // Counterparty locks the same USDC mint (same vault USDC ATA).
    let join = join_ix(
        &counterparty.pubkey(),
        AssetType::Token,
        &usdc.pubkey(),
        cp_lock,
        &covenant,
        &vault,
        &cp_usdc_holder,
        &vault_usdc,
        &usdc.pubkey(),
    );
    assert_tx_ok(&mut svm, &counterparty, &[join]);

    assert_eq!(token_balance(&svm, &vault_usdc), creator_lock + cp_lock);

    // Payout ATAs (= the existing holder ATAs).
    let creator_payout = creator_usdc;
    let cp_payout = cp_usdc_holder;

    // Simulate ER-side acceptance before settling.
    activate_covenant(&mut svm, &covenant);

    // Payout ATAs double as the holders, so capture pre-swap deltas.
    let creator_payout_before = token_balance(&svm, &creator_payout);
    let cp_payout_before = token_balance(&svm, &cp_payout);

    let swap = swap_ix(
        &covenant,
        &vault,
        &creator.pubkey(),
        &counterparty.pubkey(),
        &covenant,
        &vault_usdc,
        &vault_usdc,
        &creator_payout,
        &cp_payout,
        &usdc.pubkey(),
        &usdc.pubkey(),
        &system_program::ID,
    );
    assert_tx_ok(&mut svm, &creator, &[swap]);

    assert_eq!(token_balance(&svm, &vault_usdc), 0, "vault empties");
    // Each side receives the other's locked amount.
    assert_eq!(
        token_balance(&svm, &creator_payout),
        creator_payout_before + cp_lock
    );
    assert_eq!(
        token_balance(&svm, &cp_payout),
        cp_payout_before + creator_lock
    );
}

#[test]
fn test_usdc_sol_expire_refund() {
    let mut svm = setup_program();

    let creator = Keypair::new();
    let counterparty = Keypair::new();
    svm.airdrop(&creator.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
    svm.airdrop(&counterparty.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

    let (usdc, creator_usdc) = setup_token(&mut svm, &creator, &creator.pubkey(), 6, 5_000);
    let usdc_locked = 999u64;
    let cp_sol = 1 * LAMPORTS_PER_SOL;

    let (covenant, _) = covenant_pda(&creator.pubkey(), 0);
    let (vault, _) = vault_pda(&covenant);
    let vault_usdc = create_ata(&mut svm, &creator, &vault, &usdc.pubkey());

    let init = initialize_ix(
        &creator.pubkey(),
        Some(&counterparty.pubkey()),
        AssetType::Token,
        &usdc.pubkey(),
        usdc_locked,
        current_ts(&svm) + 1000,
        0,
        &covenant,
        &vault,
        &creator_usdc,
        &vault_usdc,
        &usdc.pubkey(),
    );
    assert_tx_ok(&mut svm, &creator, &[init]);

    let join = join_ix(
        &counterparty.pubkey(),
        AssetType::Sol,
        &system_program::ID,
        cp_sol,
        &covenant,
        &vault,
        &vault,
        &vault,
        &system_program::ID,
    );
    assert_tx_ok(&mut svm, &counterparty, &[join]);

    // Warp past deadline.
    let mut clock = svm.get_sysvar::<Clock>();
    clock.unix_timestamp += 10_000;
    svm.set_sysvar::<Clock>(&clock);

    let expire = expire_ix(
        &creator.pubkey(),
        &covenant,
        &vault,
        &creator.pubkey(),
        &counterparty.pubkey(),
        &vault_usdc,
        &vault,            // counterparty vault ATA (SOL side, unused but mut)
        &creator_usdc, // creator's own USDC ATA (payout)
        &vault,            // counterparty payout ATA (SOL side, unused but mut)
        &usdc.pubkey(),
        &system_program::ID,
    );
    assert_tx_ok(&mut svm, &creator, &[expire]);

    // Creator's USDC returned; vault emptied.
    assert_eq!(token_balance(&svm, &vault_usdc), 0);
    assert_eq!(token_balance(&svm, &creator_usdc), 5_000, "creator refunded USDC");
    // Counterparty recovers their SOL.
    assert!(svm.get_balance(&counterparty.pubkey()).unwrap() >= cp_sol);
}

#[test]
fn test_nft_nft_swap() {
    let mut svm = setup_program();

    let creator = Keypair::new();
    let counterparty = Keypair::new();
    svm.airdrop(&creator.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
    svm.airdrop(&counterparty.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

    let (nft_a, creator_a_ata) = setup_token(&mut svm, &creator, &creator.pubkey(), 0, 1);
    let (nft_b, cp_b_ata) = setup_token(&mut svm, &counterparty, &counterparty.pubkey(), 0, 1);

    let (covenant, _) = covenant_pda(&creator.pubkey(), 0);
    let (vault, _) = vault_pda(&covenant);
    let vault_a = create_ata(&mut svm, &creator, &vault, &nft_a.pubkey());
    let vault_b = create_ata(&mut svm, &creator, &vault, &nft_b.pubkey());

    let init = initialize_ix(
        &creator.pubkey(),
        Some(&counterparty.pubkey()),
        AssetType::Nft,
        &nft_a.pubkey(),
        1,
        current_ts(&svm) + 1000,
        0,
        &covenant,
        &vault,
        &creator_a_ata,
        &vault_a,
        &nft_a.pubkey(),
    );
    assert_tx_ok(&mut svm, &creator, &[init]);

    let join = join_ix(
        &counterparty.pubkey(),
        AssetType::Nft,
        &nft_b.pubkey(),
        1,
        &covenant,
        &vault,
        &cp_b_ata,
        &vault_b,
        &nft_b.pubkey(),
    );
    assert_tx_ok(&mut svm, &counterparty, &[join]);

    assert_eq!(token_balance(&svm, &vault_a), 1);
    assert_eq!(token_balance(&svm, &vault_b), 1);

    // Payout ATAs.
    let cp_a_ata = create_ata(&mut svm, &counterparty, &counterparty.pubkey(), &nft_a.pubkey());
    let creator_b_ata = create_ata(&mut svm, &creator, &creator.pubkey(), &nft_b.pubkey());

    // Simulate ER-side acceptance before settling.
    activate_covenant(&mut svm, &covenant);

    let swap = swap_ix(
        &covenant,
        &vault,
        &creator.pubkey(),
        &counterparty.pubkey(),
        &covenant,
        &vault_a,
        &vault_b,
        &creator_b_ata, // creator receives NFT b
        &cp_a_ata,      // counterparty receives NFT a
        &nft_a.pubkey(),
        &nft_b.pubkey(),
        &system_program::ID,
    );
    assert_tx_ok(&mut svm, &creator, &[swap]);

    assert_eq!(token_balance(&svm, &vault_a), 0);
    assert_eq!(token_balance(&svm, &vault_b), 0);
    assert_eq!(token_balance(&svm, &cp_a_ata), 1, "cp got NFT a");
    assert_eq!(token_balance(&svm, &creator_b_ata), 1, "creator got NFT b");
}

#[test]
fn test_open_join_then_cancel() {
    let mut svm = setup_program();

    let creator = Keypair::new();
    let taker = Keypair::new();
    let intruder = Keypair::new();
    svm.airdrop(&creator.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
    svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
    svm.airdrop(&intruder.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

    let creator_sol = 2 * LAMPORTS_PER_SOL;
    let taker_sol = 1 * LAMPORTS_PER_SOL;

    let (covenant, _) = covenant_pda(&creator.pubkey(), 0);
    let (vault, _) = vault_pda(&covenant);

    // Open covenant: no pinned counterparty.
    let init = initialize_ix(
        &creator.pubkey(),
        None,
        AssetType::Sol,
        &system_program::ID,
        creator_sol,
        current_ts(&svm) + 1000,
        0,
        &covenant,
        &vault,
        &vault,
        &vault,
        &system_program::ID,
    );
    assert_tx_ok(&mut svm, &creator, &[init]);

    // First taker joins.
    let join = join_ix(
        &taker.pubkey(),
        AssetType::Sol,
        &system_program::ID,
        taker_sol,
        &covenant,
        &vault,
        &vault,
        &vault,
        &system_program::ID,
    );
    assert_tx_ok(&mut svm, &taker, &[join]);

    // A second joiner must be rejected now that the covenant is taken.
    let join2 = join_ix(
        &intruder.pubkey(),
        AssetType::Sol,
        &system_program::ID,
        taker_sol,
        &covenant,
        &vault,
        &vault,
        &vault,
        &system_program::ID,
    );
    let blockhash = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(
        &[join2],
        Some(&intruder.pubkey()),
        &[&intruder],
        blockhash,
    );
    assert!(svm.send_transaction(tx).is_err(), "second join must fail");

    assert_eq!(
        svm.get_account(&vault).unwrap().lamports,
        creator_sol + taker_sol
    );

    // Creator cancels before delegation: both locked amounts refunded.
    let cancel = cancel_ix(
        &creator.pubkey(),
        &covenant,
        &vault,
        &creator.pubkey(),
        &taker.pubkey(),
        &vault,
        &vault,
        &vault,
        &vault,
        &system_program::ID,
        &system_program::ID,
    );
    assert_tx_ok(&mut svm, &creator, &[cancel]);

    let vault_acct = svm.get_account(&vault);
    assert!(
        vault_acct.is_none() || vault_acct.unwrap().lamports == 0,
        "vault should be empty after cancel"
    );
    assert!(svm.get_balance(&creator.pubkey()).unwrap() >= creator_sol);
    assert!(svm.get_balance(&taker.pubkey()).unwrap() >= taker_sol);

    // Terminal Reverted state can be closed to reclaim the PDA rent.
    let close = close_ix(&creator.pubkey(), &covenant);
    assert_tx_ok(&mut svm, &creator, &[close]);
    assert!(svm.get_account(&covenant).is_none(), "covenant closed");
}

#[test]
fn test_invite_only_join_rejects_wrong_party() {
    let mut svm = setup_program();

    let creator = Keypair::new();
    let invited = Keypair::new();
    let wrong = Keypair::new();
    svm.airdrop(&creator.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
    svm.airdrop(&invited.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
    svm.airdrop(&wrong.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

    let (covenant, _) = covenant_pda(&creator.pubkey(), 0);
    let (vault, _) = vault_pda(&covenant);

    let init = initialize_ix(
        &creator.pubkey(),
        Some(&invited.pubkey()),
        AssetType::Sol,
        &system_program::ID,
        2 * LAMPORTS_PER_SOL,
        current_ts(&svm) + 1000,
        0,
        &covenant,
        &vault,
        &vault,
        &vault,
        &system_program::ID,
    );
    assert_tx_ok(&mut svm, &creator, &[init]);

    // Non-invited party cannot join a pinned covenant.
    let join_wrong = join_ix(
        &wrong.pubkey(),
        AssetType::Sol,
        &system_program::ID,
        1 * LAMPORTS_PER_SOL,
        &covenant,
        &vault,
        &vault,
        &vault,
        &system_program::ID,
    );
    let blockhash = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(
        &[join_wrong],
        Some(&wrong.pubkey()),
        &[&wrong],
        blockhash,
    );
    assert!(
        svm.send_transaction(tx).is_err(),
        "wrong party must not join a pinned covenant"
    );

    // The invited party can.
    let join_ok = join_ix(
        &invited.pubkey(),
        AssetType::Sol,
        &system_program::ID,
        1 * LAMPORTS_PER_SOL,
        &covenant,
        &vault,
        &vault,
        &vault,
        &system_program::ID,
    );
    assert_tx_ok(&mut svm, &invited, &[join_ok]);
}
