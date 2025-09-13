#!/bin/bash
# Redeploy the convenant program, clearing stale upgrade buffers first.
#
# `anchor deploy` allocates an upgrade buffer sized for the new program. When a
# previous attempt was interrupted the buffer survives and the next run fails
# with "insufficient lamports / account already in use". Closing the buffers
# owned by the upgrade authority first makes redeploys idempotent.
set -euo pipefail

cd "$(dirname "$0")/.."
RPC="${SOLANA_RPC:-https://rpc.magicblock.app/devnet}"
AUTHORITY="${SOLANA_AUTHORITY:-$HOME/.config/solana/id.json}"
FUNDER="${SOLANA_FUNDER:-$HOME/.config/solana/id2.json}"

# The upgrade buffer holds ~2x rent for the program size (~2.5 SOL), so the
# authority needs comfortably more than that before deploying.
AUTH=DztJxBybR7fKa9UyR8fBZMohaZYcLydyxrJr5CuJBa34
NEEDED="${MIN_AUTHORITY_SOL:-4}"
BAL=$(solana balance "$AUTH" --url "$RPC" | tr -d ' SOL')
echo "upgrade-authority balance: $BAL SOL (want >= $NEEDED)"
if awk "BEGIN{exit !($BAL < $NEEDED)}"; then
  TOPUP=$(awk "BEGIN{print $NEEDED - $BAL + 0.5}")
  echo "topping up the upgrade authority by $TOPUP SOL from $FUNDER"
  solana transfer "$AUTH" "$TOPUP" --keypair "$FUNDER" --url "$RPC" --no-wait >/dev/null
  sleep 8
  solana balance "$AUTH" --url "$RPC"
fi

echo "closing stale upgrade buffers"
solana program close --buffers --url "$RPC" --keypair "$AUTHORITY" \
  2uY6gVNHe62XZXByt796pbwye1p7eTu2Nftv7uwC5CtQ --no-wait 2>&1 | tail -1 || true
sleep 5

echo "building"
(cd anchor && anchor build --ignore-keys 2>&1 | grep -E "Finished|error" | tail -2)

echo "deploying"
(cd anchor && anchor deploy 2>&1 | tail -4)

solana program show 2uY6gVNHe62XZXByt796pbwye1p7eTu2Nftv7uwC5CtQ --url "$RPC" \
  | grep -E "Last Deployed|Authority"
