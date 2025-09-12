use anchor_lang::prelude::*;
use sha2::{Digest as _, Sha256};

pub const COVENANT_SEED: &[u8] = b"covenant";
pub const VAULT_SEED: &[u8] = b"vault";

/// The kind of asset a party is offering.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, InitSpace, Debug)]
pub enum AssetType {
    /// Native SOL, held as lamports in the vault PDA.
    Sol,
    /// SPL/SPL-token-2022 token, held in a vault-owned token account.
    Token,
    /// NFT / metadata token (the same SPL path; amount is 1), kept distinct
    /// so the UI and settlement can display it as an NFT.
    Nft,
}

/// Lifecycle state of a covenant.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, InitSpace, Debug)]
pub enum CovenantStatus {
    /// Covenant created on base layer; creator has locked their assets.
    CreatedOnL1,
    /// Delegation transaction submitted; moving to the ER.
    Delegating,
    /// Active on the ephemeral rollup; parties can negotiate.
    ActiveOnEr,
    /// Both parties accepted; commit + swap in progress.
    Settling,
    /// Swap executed and committed back to base layer. Terminal.
    SettledOnL1,
    /// Settlement failed / reverted. Terminal (recovery path in later version).
    Reverted,
}

/// The canonical agreement escrow account. Lives as a PDA.
#[account]
#[derive(InitSpace)]
pub struct Covenant {
    pub version: u8,

    pub creator: Pubkey,
    /// The counterparty. `None` while the covenant is openly joinable; set on
    /// first join (or pinned at creation for invite-only covenants).
    pub counterparty: Option<Pubkey>,

    /// Whether the counterparty has joined and locked their assets yet.
    pub counterparty_joined: bool,

    /// Creator's locked amount. For SOL this is lamports; Token/Nft raw amount.
    pub creator_amount: u64,
    pub creator_asset: AssetType,
    /// system_program::ID for SOL, otherwise the token mint.
    pub creator_mint: Pubkey,

    /// Counterparty's locked amount.
    pub counterparty_amount: u64,
    pub counterparty_asset: AssetType,
    /// system_program::ID for SOL, otherwise the token mint.
    pub counterparty_mint: Pubkey,

    /// Acceptance flags — both parties must accept before the swap runs.
    pub creator_accepted: bool,
    pub counterparty_accepted: bool,

    /// Every snapshot keeps updating terms_hash so clients can tell a new
    /// round happened without trusting local state.
    pub terms_hash: [u8; 32],

    pub status: CovenantStatus,
    pub deadline: i64,

    pub nonce: u64,
    pub bump: u8,
    pub vault_bump: u8,

    pub created_at: i64,
    pub settled_at: Option<i64>,
}

impl Covenant {
    pub fn both_accepted(&self) -> bool {
        self.creator_accepted && self.counterparty_accepted
    }

    pub fn is_party(&self, key: &Pubkey) -> bool {
        &self.creator == key || self.counterparty.as_ref() == Some(key)
    }

    /// Canonical hash input for the currently committed offers + deadline.
    /// Solana programs can't allocate, so we hash a fixed 176-byte buffer.
    pub fn hash_terms(&self) -> [u8; 32] {
        let mut buf = [0u8; 176];
        buf[0..32].copy_from_slice(&self.creator_mint.to_bytes());
        buf[32..64].copy_from_slice(&self.counterparty_mint.to_bytes());
        buf[64..72].copy_from_slice(&self.creator_amount.to_le_bytes());
        buf[72..80].copy_from_slice(&self.counterparty_amount.to_le_bytes());
        buf[80] = self.creator_asset as u8;
        buf[81] = self.counterparty_asset as u8;
        buf[82..90].copy_from_slice(&self.deadline.to_le_bytes());
        buf[90..98].copy_from_slice(&self.nonce.to_le_bytes());
        Sha256::digest(&buf).into()
    }
}

impl Default for CovenantStatus {
    fn default() -> Self {
        CovenantStatus::CreatedOnL1
    }
}