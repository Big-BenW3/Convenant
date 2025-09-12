import {
  isSolanaError,
  SOLANA_ERROR__INSTRUCTION_ERROR__CUSTOM,
} from "@solana/kit";
import {
  getConvenantErrorMessage,
  type ConvenantError,
} from "../generated/convenant";

/**
 * Every custom error code the program can raise.
 *
 * The generated client only ships its message table in development builds
 * (`process.env.NODE_ENV !== "production"`), so this list is spelled out here
 * rather than derived from the generated table: decoding has to keep working in
 * the production bundle that Vercel serves.
 */
const enum ConvenantErrorCode {
  UNAUTHORIZED_SIGNER = 0x1770,
  INVALID_STATUS = 0x1771,
  DEADLINE_IN_PAST = 0x1772,
  ZERO_AMOUNT = 0x1773,
  SAME_PARTY = 0x1774,
  INVALID_COUNTERPARTY = 0x1775,
  COUNTERPARTY_NOT_JOINED = 0x1776,
  COUNTERPARTY_ALREADY_JOINED = 0x1777,
  NOT_BOTH_ACCEPTED = 0x1778,
  DEADLINE_NOT_PASSED = 0x1779,
  INSUFFICIENT_FUNDS = 0x177a,
  TERMS_MISMATCH = 0x177b,
  ALREADY_SETTLED = 0x177c,
  UNSUPPORTED_ASSET = 0x177d,
  INVALID_NFT_MINT = 0x177e,
  MISSING_MINT = 0x177f,
  VAULT_MISMATCH = 0x1780,
  UNSUPPORTED_VERSION = 0x1781,
  FROZEN_AFTER_ACCEPT = 0x1782,
  INVALID_PERMISSION_PDA = 0x1783,
  INVALID_MINT = 0x1784,
  INVALID_TOKEN_PROGRAM = 0x1785,
  COVENANT_PDA_MISMATCH = 0x1786,
  CREATOR_MISMATCH = 0x1787,
  COUNTERPARTY_MISMATCH = 0x1788,
  ESCROW_AUTHORITY_MISMATCH = 0x1789,
}

/**
 * Human-readable copy for codes whose generated wording is accurate but reads
 * like program output rather than an explanation.
 */
const FRIENDLY_ERRORS: Partial<Record<ConvenantError, string>> = {
  [ConvenantErrorCode.UNAUTHORIZED_SIGNER]:
    "You are not a party to this covenant.",
  [ConvenantErrorCode.INVALID_STATUS]:
    "This covenant can no longer be changed in its current state.",
  [ConvenantErrorCode.DEADLINE_IN_PAST]:
    "The deadline has already passed, so this can no longer be changed.",
  [ConvenantErrorCode.ZERO_AMOUNT]: "Enter an amount greater than zero.",
  [ConvenantErrorCode.SAME_PARTY]:
    "You cannot be both parties to the same covenant.",
  [ConvenantErrorCode.COUNTERPARTY_ALREADY_JOINED]:
    "Someone has already joined this covenant.",
  [ConvenantErrorCode.COUNTERPARTY_NOT_JOINED]:
    "Waiting for the other party to join before this can proceed.",
  [ConvenantErrorCode.NOT_BOTH_ACCEPTED]:
    "Both parties must accept the current terms before settling.",
  [ConvenantErrorCode.DEADLINE_NOT_PASSED]:
    "The deadline has not passed yet, so a refund is not available.",
  [ConvenantErrorCode.INSUFFICIENT_FUNDS]:
    "That amount is more than this covenant currently holds in escrow.",
  [ConvenantErrorCode.FROZEN_AFTER_ACCEPT]:
    "You already accepted these terms. Change the amount before accepting again.",
  [ConvenantErrorCode.INVALID_MINT]: "That token mint could not be read.",
  [ConvenantErrorCode.MISSING_MINT]:
    "Choose a token to offer, rather than SOL.",
  [ConvenantErrorCode.INVALID_TOKEN_PROGRAM]: "Unsupported token program.",
  [ConvenantErrorCode.COVENANT_PDA_MISMATCH]:
    "This covenant address does not match its recorded seeds.",
  [ConvenantErrorCode.CREATOR_MISMATCH]:
    "The payout account does not match this covenant's creator.",
  [ConvenantErrorCode.COUNTERPARTY_MISMATCH]:
    "The payout account does not match this covenant's counterparty.",
  [ConvenantErrorCode.ESCROW_AUTHORITY_MISMATCH]:
    "This settlement was not authorised by the covenant escrow.",
  [ConvenantErrorCode.ALREADY_SETTLED]: "This covenant has already settled.",
  [ConvenantErrorCode.TERMS_MISMATCH]:
    "The terms changed since you accepted. Review the new amount.",
};

const CONVENANT_ERROR_CODES: ReadonlySet<number> = new Set<number>([
  ConvenantErrorCode.UNAUTHORIZED_SIGNER,
  ConvenantErrorCode.INVALID_STATUS,
  ConvenantErrorCode.DEADLINE_IN_PAST,
  ConvenantErrorCode.ZERO_AMOUNT,
  ConvenantErrorCode.SAME_PARTY,
  ConvenantErrorCode.INVALID_COUNTERPARTY,
  ConvenantErrorCode.COUNTERPARTY_NOT_JOINED,
  ConvenantErrorCode.COUNTERPARTY_ALREADY_JOINED,
  ConvenantErrorCode.NOT_BOTH_ACCEPTED,
  ConvenantErrorCode.DEADLINE_NOT_PASSED,
  ConvenantErrorCode.INSUFFICIENT_FUNDS,
  ConvenantErrorCode.TERMS_MISMATCH,
  ConvenantErrorCode.ALREADY_SETTLED,
  ConvenantErrorCode.UNSUPPORTED_ASSET,
  ConvenantErrorCode.INVALID_NFT_MINT,
  ConvenantErrorCode.MISSING_MINT,
  ConvenantErrorCode.VAULT_MISMATCH,
  ConvenantErrorCode.UNSUPPORTED_VERSION,
  ConvenantErrorCode.FROZEN_AFTER_ACCEPT,
  ConvenantErrorCode.INVALID_PERMISSION_PDA,
  ConvenantErrorCode.INVALID_MINT,
  ConvenantErrorCode.INVALID_TOKEN_PROGRAM,
  ConvenantErrorCode.COVENANT_PDA_MISMATCH,
  ConvenantErrorCode.CREATOR_MISMATCH,
  ConvenantErrorCode.COUNTERPARTY_MISMATCH,
  ConvenantErrorCode.ESCROW_AUTHORITY_MISMATCH,
]);

export function parseTransactionError(err: unknown): string {
  // Wallet rejection (comes from wallet-standard, not a SolanaError).
  if (err instanceof Error && err.message.includes("User rejected")) {
    return "Transaction was rejected by the wallet.";
  }

  // Anchor custom program errors — decode via the generated convenant client.
  if (
    isSolanaError(err, SOLANA_ERROR__INSTRUCTION_ERROR__CUSTOM) &&
    typeof err.context?.code === "number"
  ) {
    const code = err.context.code;
    if (CONVENANT_ERROR_CODES.has(code)) {
      const convenantError = code as ConvenantError;
      return (
        FRIENDLY_ERRORS[convenantError] ??
        getConvenantErrorMessage(convenantError)
      );
    }
  }

  // For all other errors, kit's SolanaError already has readable messages.
  // Walk the cause chain to find the deepest message.
  const message = getDeepestMessage(err);
  return message.length > 200 ? `${message.slice(0, 200)}...` : message;
}

function getDeepestMessage(err: unknown): string {
  let deepest = err instanceof Error ? err.message : String(err);
  let current: unknown = err;

  while (current instanceof Error && current.cause) {
    current = current.cause;
    if (current instanceof Error) {
      deepest = current.message;
    }
  }

  return deepest;
}
