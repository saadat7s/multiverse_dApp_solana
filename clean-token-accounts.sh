#!/bin/bash

# Configuration
KEYPAIR="hamad-wallet-2-keypair.json"
OWNER=$(solana address -k "$KEYPAIR")
MINT="mLSeR1QWF2Ay4rZDiU6o61BZxvbQd2LThJoCYmQHwEg"

# Get all token accounts for the mint and owner
accounts=($(spl-token accounts --owner "$OWNER" | grep "$MINT" | awk '{print $1}'))

# Exit if no accounts found
if [ ${#accounts[@]} -eq 0 ]; then
  echo "No token accounts found for mint $MINT and owner $OWNER"
  exit 1
fi

# Pick the first as the main account
PRIMARY_ACCOUNT=${accounts[0]}
echo "Primary token account: $PRIMARY_ACCOUNT"
echo "All token accounts: ${accounts[@]}"
echo

# Loop through the rest
for acct in "${accounts[@]:1}"; do
  balance=$(spl-token balance "$acct" --owner "$KEYPAIR")
  
  # If balance > 0, transfer to primary account
  if (( $(echo "$balance > 0" | bc -l) )); then
    echo "Transferring $balance from $acct to $PRIMARY_ACCOUNT"
    spl-token transfer "$MINT" "$balance" "$PRIMARY_ACCOUNT" --fund-recipient --owner "$KEYPAIR" --from "$acct"
  fi
  
  # Close the account
  echo "Closing token account: $acct"
  spl-token close "$acct" --owner "$KEYPAIR"
done

echo
echo "✅ Cleanup complete. Kept account: $PRIMARY_ACCOUNT"
