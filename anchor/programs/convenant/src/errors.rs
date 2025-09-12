use anchor_lang::prelude::*;

#[error_code]
pub enum ConvenantError {
    #[msg("Unauthorized signer")]
    UnauthorizedSigner,
    #[msg("Cannot act on a covenant in this status")]
    InvalidStatus,
    #[msg("Deadline must be in the future")]
    DeadlineInPast,
    #[msg("Amount must be nonzero")]
    ZeroAmount,
    #[msg("Counterparty cannot be the creator")]
    SameParty,
    #[msg("Counterparty cannot be the system program")]
    InvalidCounterparty,
    #[msg("The counterparty has not joined yet")]
    CounterpartyNotJoined,
    #[msg("The counterparty has already joined")]
    CounterpartyAlreadyJoined,
    #[msg("Both parties must accept before swap")]
    NotBothAccepted,
    #[msg("Deadline has not passed yet")]
    DeadlineNotPassed,
    #[msg("Insufficient funds for this asset")]
    InsufficientFunds,
    #[msg("Terms no longer match the committed offers")]
    TermsMismatch,
    #[msg("Covenant already settled")]
    AlreadySettled,
    #[msg("Unsupported asset for settling from vault")]
    UnsupportedAsset,
    #[msg("NFT mint supplied but asset is not an NFT")]
    InvalidNftMint,
    #[msg("A token mint is required for token/NFT assets")]
    MissingMint,
    #[msg("The provided vault does not match the covenant")]
    VaultMismatch,
    #[msg("Unsupported account version")]
    UnsupportedVersion,
    #[msg("Supply cannot be altered after acceptance")]
    FrozenAfterAccept,
    #[msg("The provided permission account does not match the covenant")]
    InvalidPermissionPda,
    #[msg("The provided mint account is invalid")]
    InvalidMint,
    #[msg("The provided token program is invalid")]
    InvalidTokenProgram,
    #[msg("The provided covenant PDA does not match the covenant seeds")]
    CovenantPdaMismatch,
    #[msg("The provided creator account does not match the covenant")]
    CreatorMismatch,
    #[msg("The provided counterparty account does not match the covenant")]
    CounterpartyMismatch,
    #[msg("The provided escrow authority is not the covenant PDA")]
    EscrowAuthorityMismatch,
}