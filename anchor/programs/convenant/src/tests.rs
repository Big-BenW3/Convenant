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
const DISC_UPDATE_OFFER: [u8; 8] = [191, 70, 15, 66, 224, 2, 249, 223];
const DISC_ACCEPT: [u8; 8] = [65, 150, 70, 216, 133, 6, 107, 4];

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

/// `update_offer` / `accept` share the `Negotiate` context: payer (signer) and
/// covenant. The vault is deliberately absent — it is not delegated to the ER,
/// so it cannot be an account of an ER-side instruction.
fn negotiate_ix(payer: &Pubkey, covenant: &Pubkey, disc: [u8; 8], amount: Option<u64>) -> Instruction {
    let mut data = disc.to_vec();
    if let Some(a) = amount {
        data.extend_from_slice(&a.to_le_bytes());
    }
    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(*covenant, false),
        ],
        data,
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

// ---------------------------------------------------------------------
// Security regression tests
//
// Every test here covers a property that was previously unenforced. They are
// deliberately negative: each asserts that a specific abuse is REJECTED and
// that the vault is left intact afterwards.
// ---------------------------------------------------------------------

/// Anchor user error codes (offset 6000), matching the declaration order in
/// `errors.rs`. Kept as named constants so a reorder surfaces as a test failure
/// rather than a silently wrong assertion.
const ERR_UNAUTHORIZED_SIGNER: u32 = 6000;
const ERR_DEADLINE_IN_PAST: u32 = 6002;
const ERR_ZERO_AMOUNT: u32 = 6003;
const ERR_COUNTERPARTY_NOT_JOINED: u32 = 6006;
const ERR_NOT_BOTH_ACCEPTED: u32 = 6008;
const ERR_DEADLINE_NOT_PASSED: u32 = 6009;
const ERR_INSUFFICIENT_FUNDS: u32 = 6010;
const ERR_FROZEN_AFTER_ACCEPT: u32 = 6018;
const ERR_CREATOR_MISMATCH: u32 = 6023;
const ERR_COUNTERPARTY_MISMATCH: u32 = 6024;
const ERR_ESCROW_AUTHORITY_MISMATCH: u32 = 6025;

/// LiteSVM charges 5000 lamports per transaction by default. Assertions on
/// actor balances subtract this when the actor is also the fee payer.
const TX_FEE: u64 = 5000;

/// Send a transaction that must fail with the given Anchor user error code.
/// Asserting on the exact code (not merely `is_err`) is what makes these
/// regression tests meaningful: a failure for the wrong reason would pass a
/// weaker assertion.
fn assert_tx_err(svm: &mut LiteSVM, payer: &Keypair, ixs: &[Instruction], expected: u32) {
    let blockhash = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(ixs, Some(&payer.pubkey()), &[payer], blockhash);
    match svm.send_transaction(tx) {
        Ok(_) => panic!("expected transaction to fail with error {expected}, but it succeeded"),
        Err(e) => {
            let rendered = format!("{e:?}");
            let needle = format!("Custom({expected})");
            assert!(
                rendered.contains(&needle),
                "expected error {expected} ({needle}), got: {rendered}"
            );
        }
    }
}

/// Builds and funds a creator + counterparty, runs `initialize_covenant` and
/// `join_covenant` for a plain SOL<->SOL covenant, and returns the context plus
/// the locked balances.
///
/// `deadline_offset` is always positive here — `initialize_covenant` rejects a
/// deadline in the past. Tests that need an already-expired covenant call
/// `advance_clock_past_deadline` afterwards.
struct SolCovenant {
    svm: LiteSVM,
    creator: Keypair,
    counterparty: Keypair,
    covenant: Pubkey,
    vault: Pubkey,
    creator_lock: u64,
    cp_lock: u64,
    deadline: i64,
}

fn setup_sol_covenant(deadline_offset: i64) -> SolCovenant {
    let mut svm = setup_program();
    let creator = Keypair::new();
    let counterparty = Keypair::new();
    svm.airdrop(&creator.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
    svm.airdrop(&counterparty.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

    let creator_lock = 2 * LAMPORTS_PER_SOL;
    let cp_lock = 1 * LAMPORTS_PER_SOL;
    let (covenant, _) = covenant_pda(&creator.pubkey(), 0);
    let (vault, _) = vault_pda(&covenant);

    let deadline = current_ts(&svm) + deadline_offset;
    let init = initialize_ix(
        &creator.pubkey(),
        Some(&counterparty.pubkey()),
        AssetType::Sol,
        &system_program::ID,
        creator_lock,
        deadline,
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
        cp_lock,
        &covenant,
        &vault,
        &vault,
        &vault,
        &system_program::ID,
    );
    assert_tx_ok(&mut svm, &counterparty, &[join]);

    assert_eq!(
        svm.get_account(&vault).unwrap().lamports,
        creator_lock + cp_lock,
        "vault should hold both locked amounts"
    );

    SolCovenant {
        svm,
        creator,
        counterparty,
        covenant,
        vault,
        creator_lock,
        cp_lock,
        deadline,
    }
}

/// Move LiteSVM's clock past the covenant deadline so `expire_and_refund`
/// becomes reachable. LiteSVM's clock lives in the Clock sysvar account.
fn advance_clock_past_deadline(svm: &mut LiteSVM, deadline: i64) {
    let mut acct = svm.get_account(&CLOCK_ID).unwrap();
    // unix_ts is at byte offset 32 within the Clock account data.
    acct.data[32..40].copy_from_slice(&(deadline + 10).to_le_bytes());
    svm.set_account(CLOCK_ID, acct).unwrap();
}

/// Balance delta helper so refund assertions are exact rather than `>=`.
fn delta(before: u64, after: u64) -> i128 {
    after as i128 - before as i128
}

/// The original fund-theft bug: `expire_and_refund` declared its recipients as
/// unconstrained `UncheckedAccount`s, so the counterparty could name an
/// attacker-controlled `creator` and have the vault PDA sign the creator's
/// locked SOL over to them.
#[test]
fn test_expire_rejects_attacker_controlled_creator_recipient() {
    let mut c = setup_sol_covenant(60);
    advance_clock_past_deadline(&mut c.svm, c.deadline);
    let mut svm = c.svm;
    let attacker = Keypair::new();
    svm.airdrop(&attacker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

    let attacker_before = svm.get_balance(&attacker.pubkey()).unwrap();

    // Counterparty signs (passes the party check) but redirects the creator's
    // refund to the attacker.
    let ix = expire_ix(
        &c.counterparty.pubkey(),
        &c.covenant,
        &c.vault,
        &attacker.pubkey(), // <- forged creator
        &c.counterparty.pubkey(),
        &c.vault,
        &c.vault,
        &c.vault,
        &c.vault,
        &system_program::ID,
        &system_program::ID,
    );
    assert_tx_err(&mut svm, &c.counterparty, &[ix], ERR_CREATOR_MISMATCH);

    // Nothing moved: the attacker's balance is unchanged and the vault is intact.
    // The counterparty signed, so its only movement is the transaction fee.
    // The attacker never appears in the transaction at all.
    let attacker_after = svm.get_balance(&attacker.pubkey()).unwrap();
    assert_eq!(
        delta(attacker_before, attacker_after),
        0,
        "attacker must not receive any funds from a forged refund"
    );
    assert_eq!(
        svm.get_account(&c.vault).unwrap().lamports,
        c.creator_lock + c.cp_lock,
        "vault must be untouched after a rejected forged refund"
    );
}

/// Mirror of the above for `cancel_covenant`: the creator could previously name
/// an arbitrary `counterparty` and take the counterparty's locked SOL.
#[test]
fn test_cancel_rejects_attacker_controlled_counterparty_recipient() {
    let c = setup_sol_covenant(1000);
    let mut svm = c.svm;
    let attacker = Keypair::new();
    svm.airdrop(&attacker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

    let attacker_before = svm.get_balance(&attacker.pubkey()).unwrap();

    let ix = cancel_ix(
        &c.creator.pubkey(),
        &c.covenant,
        &c.vault,
        &c.creator.pubkey(),
        &attacker.pubkey(), // <- forged counterparty
        &c.vault,
        &c.vault,
        &c.vault,
        &c.vault,
        &system_program::ID,
        &system_program::ID,
    );
    assert_tx_err(&mut svm, &c.creator, &[ix], ERR_COUNTERPARTY_MISMATCH);

    let attacker_after = svm.get_balance(&attacker.pubkey()).unwrap();
    assert_eq!(
        delta(attacker_before, attacker_after),
        0,
        "attacker must not receive the counterparty's locked funds"
    );
    assert_eq!(
        svm.get_account(&c.vault).unwrap().lamports,
        c.creator_lock + c.cp_lock,
        "vault must be untouched after a rejected forged cancel"
    );
}

/// `swap_on_l1` must not settle to caller-chosen accounts. Because both
/// recipients were unconstrained, anyone could drain the entire vault into two
/// wallets they control.
#[test]
fn test_swap_rejects_attacker_controlled_recipients() {
    let c = setup_sol_covenant(1000);
    let mut svm = c.svm;
    let attacker_a = Keypair::new();
    let attacker_b = Keypair::new();
    svm.airdrop(&attacker_a.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
    svm.airdrop(&attacker_b.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

    let a_before = svm.get_balance(&attacker_a.pubkey()).unwrap();
    let b_before = svm.get_balance(&attacker_b.pubkey()).unwrap();

    activate_covenant(&mut svm, &c.covenant);

    // Forged creator recipient.
    let ix = swap_ix(
        &c.covenant,
        &c.vault,
        &attacker_a.pubkey(), // <- forged creator
        &attacker_b.pubkey(), // <- forged counterparty
        &c.covenant,
        &c.vault,
        &c.vault,
        &c.vault,
        &c.vault,
        &system_program::ID,
        &system_program::ID,
        &system_program::ID,
    );
    assert_tx_err(&mut svm, &attacker_a, &[ix], ERR_CREATOR_MISMATCH);

    // attacker_a signed (it is the declared payer), so it loses only the fee.
    // attacker_b is never referenced by the transaction.
    assert_eq!(
        delta(a_before, svm.get_balance(&attacker_a.pubkey()).unwrap()),
        -(TX_FEE as i128),
        "the signing attacker must gain nothing but a fee charge"
    );
    assert_eq!(
        delta(b_before, svm.get_balance(&attacker_b.pubkey()).unwrap()),
        0,
        "the second attacker must gain nothing"
    );
    assert_eq!(
        svm.get_account(&c.vault).unwrap().lamports,
        c.creator_lock + c.cp_lock,
        "vault must be untouched when settlement targets forged recipients"
    );
}

/// The escrow authority must be the covenant PDA. Passing an arbitrary key used
/// to satisfy nothing beyond a key comparison the attacker could reproduce.
#[test]
fn test_swap_rejects_wrong_escrow_authority() {
    let c = setup_sol_covenant(1000);
    let mut svm = c.svm;
    let impostor = Keypair::new();
    svm.airdrop(&impostor.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

    activate_covenant(&mut svm, &c.covenant);

    let ix = swap_ix(
        &c.covenant,
        &c.vault,
        &c.creator.pubkey(),
        &c.counterparty.pubkey(),
        &impostor.pubkey(), // <- not the covenant PDA
        &c.vault,
        &c.vault,
        &c.vault,
        &c.vault,
        &system_program::ID,
        &system_program::ID,
        &system_program::ID,
    );
    assert_tx_err(&mut svm, &impostor, &[ix], ERR_ESCROW_AUTHORITY_MISMATCH);

    assert_eq!(
        svm.get_account(&c.vault).unwrap().lamports,
        c.creator_lock + c.cp_lock,
        "vault must be untouched when the escrow authority is wrong"
    );
}

/// A counterparty must not be able to refund before the deadline.
#[test]
fn test_expire_rejected_before_deadline() {
    let c = setup_sol_covenant(1000); // deadline in the future
    let mut svm = c.svm;

    let ix = expire_ix(
        &c.counterparty.pubkey(),
        &c.covenant,
        &c.vault,
        &c.creator.pubkey(),
        &c.counterparty.pubkey(),
        &c.vault,
        &c.vault,
        &c.vault,
        &c.vault,
        &system_program::ID,
        &system_program::ID,
    );
    assert_tx_err(&mut svm, &c.counterparty, &[ix], ERR_DEADLINE_NOT_PASSED);
    assert_eq!(
        svm.get_account(&c.vault).unwrap().lamports,
        c.creator_lock + c.cp_lock
    );
}

/// Only the two recorded parties may trigger a refund.
#[test]
fn test_expire_rejects_third_party() {
    let mut c = setup_sol_covenant(60);
    advance_clock_past_deadline(&mut c.svm, c.deadline);
    let mut svm = c.svm;
    let stranger = Keypair::new();
    svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

    let ix = expire_ix(
        &stranger.pubkey(),
        &c.covenant,
        &c.vault,
        &c.creator.pubkey(),
        &c.counterparty.pubkey(),
        &c.vault,
        &c.vault,
        &c.vault,
        &c.vault,
        &system_program::ID,
        &system_program::ID,
    );
    assert_tx_err(&mut svm, &stranger, &[ix], ERR_UNAUTHORIZED_SIGNER);
    assert_eq!(
        svm.get_account(&c.vault).unwrap().lamports,
        c.creator_lock + c.cp_lock
    );
}

/// Only the creator may cancel.
#[test]
fn test_cancel_rejects_non_creator() {
    let c = setup_sol_covenant(1000);
    let mut svm = c.svm;

    let ix = cancel_ix(
        &c.counterparty.pubkey(),
        &c.covenant,
        &c.vault,
        &c.creator.pubkey(),
        &c.counterparty.pubkey(),
        &c.vault,
        &c.vault,
        &c.vault,
        &c.vault,
        &system_program::ID,
        &system_program::ID,
    );
    assert_tx_err(&mut svm, &c.counterparty, &[ix], ERR_UNAUTHORIZED_SIGNER);
    assert_eq!(
        svm.get_account(&c.vault).unwrap().lamports,
        c.creator_lock + c.cp_lock
    );
}

/// `join_covenant` must be rejected once the deadline has passed.
#[test]
fn test_join_rejected_after_deadline() {
    let c = setup_sol_covenant(1000);
    let mut svm = c.svm;
    let latecomer = Keypair::new();
    svm.airdrop(&latecomer.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

    let (covenant2, _) = covenant_pda(&c.creator.pubkey(), 7);
    let (vault2, _) = vault_pda(&covenant2);
    let init = initialize_ix(
        &c.creator.pubkey(),
        Some(&latecomer.pubkey()),
        AssetType::Sol,
        &system_program::ID,
        1 * LAMPORTS_PER_SOL,
        current_ts(&svm) - 1, // already expired
        7,
        &covenant2,
        &vault2,
        &vault2,
        &vault2,
        &system_program::ID,
    );
    // initialize itself must reject a past deadline.
    assert_tx_err(&mut svm, &c.creator, &[init], ERR_DEADLINE_IN_PAST);
}

/// Flip the covenant to `ActiveOnEr` while leaving both acceptance flags
/// false, which is the state the ER actually presents once a covenant is
/// delegated but nobody has accepted yet. Use this when a test needs
/// `update_offer` to reach its amount checks rather than short-circuiting on
/// `FrozenAfterAccept`.
fn set_active_er_unaccepted(svm: &mut LiteSVM, covenant: &Pubkey) {
    let mut acct = svm.get_account(covenant).unwrap();
    acct.data[191] = 2; // CovenantStatus::ActiveOnEr
    svm.set_account(*covenant, acct).unwrap();
}

/// A party must not be able to re-price beyond what is actually escrowed.
/// Negotiation runs on the ER where the vault is not readable, so the bound is
/// `escrow_total` recorded at join: the two offers together may never exceed it.
#[test]
fn test_update_offer_rejects_insolvent_repricing() {
    let c = setup_sol_covenant(1000);
    let mut svm = c.svm;

    set_active_er_unaccepted(&mut svm, &c.covenant);

    let escrow_total = c.creator_lock + c.cp_lock;
    // The creator already offers creator_lock, so the counterparty may raise its
    // own offer by at most the creator's lock without exceeding the escrow.
    let max_cp_offer = escrow_total - c.creator_lock;
    let too_much = max_cp_offer + 1;

    let ix = negotiate_ix(
        &c.counterparty.pubkey(),
        &c.covenant,
        DISC_UPDATE_OFFER,
        Some(too_much),
    );
    assert_tx_err(&mut svm, &c.counterparty, &[ix], ERR_INSUFFICIENT_FUNDS);

    // Terms unchanged and the escrow is untouched.
    assert_eq!(
        svm.get_account(&c.vault).unwrap().lamports,
        c.creator_lock + c.cp_lock,
        "a rejected re-price must not move funds"
    );

    // Raising to exactly the escrow total is allowed.
    let ix = negotiate_ix(
        &c.counterparty.pubkey(),
        &c.covenant,
        DISC_UPDATE_OFFER,
        Some(max_cp_offer),
    );
    assert_tx_ok(&mut svm, &c.counterparty, &[ix]);
}

/// The creator side is bounded by the same rule.
#[test]
fn test_update_offer_rejects_insolvent_creator_repricing() {
    let c = setup_sol_covenant(1000);
    let mut svm = c.svm;

    set_active_er_unaccepted(&mut svm, &c.covenant);

    let escrow_total = c.creator_lock + c.cp_lock;
    let too_much = escrow_total + 1;
    let ix = negotiate_ix(
        &c.creator.pubkey(),
        &c.covenant,
        DISC_UPDATE_OFFER,
        Some(too_much),
    );
    assert_tx_err(&mut svm, &c.creator, &[ix], ERR_INSUFFICIENT_FUNDS);
}

/// Zero is rejected — and the handler must validate *before* writing, so a
/// failed update leaves the committed amount untouched.
#[test]
fn test_update_offer_rejects_zero_amount() {
    let c = setup_sol_covenant(1000);
    let mut svm = c.svm;

    set_active_er_unaccepted(&mut svm, &c.covenant);

    let ix = negotiate_ix(
        &c.counterparty.pubkey(),
        &c.covenant,
        DISC_UPDATE_OFFER,
        Some(0),
    );
    assert_tx_err(&mut svm, &c.counterparty, &[ix], ERR_ZERO_AMOUNT);
}

/// Once a party has accepted, it cannot silently re-price its own offer.
#[test]
fn test_update_offer_rejected_after_accept() {
    let c = setup_sol_covenant(1000);
    let mut svm = c.svm;

    activate_covenant(&mut svm, &c.covenant);

    // counterparty accepts, then tries to move its offer.
    let accept = negotiate_ix(
        &c.counterparty.pubkey(),
        &c.covenant,
        DISC_ACCEPT,
        None,
    );
    assert_tx_ok(&mut svm, &c.counterparty, &[accept]);

    let reprice = negotiate_ix(
        &c.counterparty.pubkey(),
        &c.covenant,
        DISC_UPDATE_OFFER,
        Some(1_500_000_000),
    );
    assert_tx_err(&mut svm, &c.counterparty, &[reprice], ERR_FROZEN_AFTER_ACCEPT);
}

/// A non-party can neither negotiate nor accept.
#[test]
fn test_negotiate_rejects_non_party() {
    let c = setup_sol_covenant(1000);
    let mut svm = c.svm;
    let stranger = Keypair::new();
    svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

    set_active_er_unaccepted(&mut svm, &c.covenant);

    let accept = negotiate_ix(
        &stranger.pubkey(),
        &c.covenant,
        DISC_ACCEPT,
        None,
    );
    assert_tx_err(&mut svm, &stranger, &[accept], ERR_UNAUTHORIZED_SIGNER);

    let reprice = negotiate_ix(
        &stranger.pubkey(),
        &c.covenant,
        DISC_UPDATE_OFFER,
        Some(500_000_000),
    );
    assert_tx_err(&mut svm, &stranger, &[reprice], ERR_UNAUTHORIZED_SIGNER);
}

/// Settlement requires mutual acceptance — one-sided acceptance must not move
/// funds out of the vault.
#[test]
fn test_swap_rejected_without_both_accepted() {
    let c = setup_sol_covenant(1000);
    let mut svm = c.svm;

    // ActiveOnEr but nobody has accepted.
    let mut acct = svm.get_account(&c.covenant).unwrap();
    acct.data[191] = 2; // CovenantStatus::ActiveOnEr
    svm.set_account(c.covenant, acct).unwrap();

    let ix = swap_ix(
        &c.covenant,
        &c.vault,
        &c.creator.pubkey(),
        &c.counterparty.pubkey(),
        &c.covenant,
        &c.vault,
        &c.vault,
        &c.vault,
        &c.vault,
        &system_program::ID,
        &system_program::ID,
        &system_program::ID,
    );
    assert_tx_err(&mut svm, &c.creator, &[ix], ERR_NOT_BOTH_ACCEPTED);
    assert_eq!(
        svm.get_account(&c.vault).unwrap().lamports,
        c.creator_lock + c.cp_lock
    );
}

/// A covenant whose counterparty never joined has no settlement path: the
/// counterparty recipient cannot be resolved.
#[test]
fn test_settlement_rejects_unjoined_counterparty() {
    let mut svm = setup_program();
    let creator = Keypair::new();
    let invitee = Keypair::new();
    let fees = Keypair::new();
    svm.airdrop(&creator.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
    svm.airdrop(&invitee.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
    svm.airdrop(&fees.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

    let lock = 2 * LAMPORTS_PER_SOL;
    let (covenant, _) = covenant_pda(&creator.pubkey(), 3);
    let (vault, _) = vault_pda(&covenant);

    // Open covenant: no counterparty is pinned and nobody joins.
    let init = initialize_ix(
        &creator.pubkey(),
        None,
        AssetType::Sol,
        &system_program::ID,
        lock,
        current_ts(&svm) + 1000,
        3,
        &covenant,
        &vault,
        &vault,
        &vault,
        &system_program::ID,
    );
    assert_tx_ok(&mut svm, &creator, &[init]);

    let ix = cancel_ix(
        &creator.pubkey(),
        &covenant,
        &vault,
        &creator.pubkey(),
        &invitee.pubkey(), // never joined
        &vault,
        &vault,
        &vault,
        &vault,
        &system_program::ID,
        &system_program::ID,
    );
    assert_tx_err(&mut svm, &creator, &[ix], ERR_COUNTERPARTY_NOT_JOINED);

    assert_eq!(
        svm.get_account(&vault).unwrap().lamports,
        lock,
        "creator's locked funds must survive a rejected cancel"
    );
}

/// Sanity check that the legitimate refund path still returns exactly the
/// locked amounts — the negative tests above must not have broken it.
#[test]
fn test_expire_refunds_exact_locked_amounts() {
    let mut c = setup_sol_covenant(60);
    advance_clock_past_deadline(&mut c.svm, c.deadline);
    let mut svm = c.svm;

    let creator_before = svm.get_balance(&c.creator.pubkey()).unwrap();
    let cp_before = svm.get_balance(&c.counterparty.pubkey()).unwrap();

    let ix = expire_ix(
        &c.creator.pubkey(),
        &c.covenant,
        &c.vault,
        &c.creator.pubkey(),
        &c.counterparty.pubkey(),
        &c.vault,
        &c.vault,
        &c.vault,
        &c.vault,
        &system_program::ID,
        &system_program::ID,
    );
    assert_tx_ok(&mut svm, &c.creator, &[ix]);

    let creator_after = svm.get_balance(&c.creator.pubkey()).unwrap();
    let cp_after = svm.get_balance(&c.counterparty.pubkey()).unwrap();

    // The creator signs and therefore pays the 5000-lamport fee; the
    // counterparty is untouched by fees. Both recover exactly their own lock.
    assert_eq!(
        delta(creator_before, creator_after),
        c.creator_lock as i128 - TX_FEE as i128,
        "creator must recover exactly their lock, less the fee they paid"
    );
    assert_eq!(
        delta(cp_before, cp_after),
        c.cp_lock as i128,
        "counterparty must recover exactly their lock"
    );
}
