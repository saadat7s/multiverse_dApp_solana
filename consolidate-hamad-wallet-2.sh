#!/bin/bash

# Quick cleanup for current situation - transfer from Aux-1 and Aux-4, then close accounts

WALLET_FILE="${1:-hamad-wallet-2-keypair.json}"
TOKEN_MINT="mLSeR1QWF2Ay4rZDiU6o61BZxvbQd2LThJoCYmQHwEg"
MAIN_ATA="ENa7qcdWbyRHgWmB3oz5Lp1qgEg5jX1nWMG7vTLK1VsQ"

echo "⚡ Quick CRD cleanup - Current situation"
echo "=============================================================================="

echo "💰 SOL balance before:"
solana balance --keypair $WALLET_FILE

echo ""
echo "📊 Current CRD balances:"
spl-token accounts --owner $WALLET_FILE | grep "$TOKEN_MINT"

echo ""
echo "🔄 Phase 1: Transfer remaining CRD tokens"

# Based on your current verbose output:
# Aux-1: GEQK8keXE4EZK5cAzNeoXgYrPPUNhH2iScKvT8y8sYhf (0.000001)
# Aux-4: FaV7gjkGbzMRQ6xvq5FnYyt3K9FzCet3eGWJAu6jYees (0.000001)

echo "   📦 Transferring 0.000001 CRD from Aux-1..."
if spl-token transfer $TOKEN_MINT 0.000001 $MAIN_ATA --owner $WALLET_FILE --from GEQK8keXE4EZK5cAzNeoXgYrPPUNhH2iScKvT8y8sYhf; then
    echo "   ✅ Transferred from Aux-1"
else
    echo "   ❌ Failed to transfer from Aux-1"
fi

echo "   📦 Transferring 0.000001 CRD from Aux-4..."
if spl-token transfer $TOKEN_MINT 0.000001 $MAIN_ATA --owner $WALLET_FILE --from FaV7gjkGbzMRQ6xvq5FnYyt3K9FzCet3eGWJAu6jYees; then
    echo "   ✅ Transferred from Aux-4"
else
    echo "   ❌ Failed to transfer from Aux-4"
fi

echo ""
echo "📊 Balances after transfers:"
spl-token accounts --owner $WALLET_FILE | grep "$TOKEN_MINT"

echo ""
echo "🗑️  Phase 2: Close auxiliary accounts"

# Current auxiliary accounts from your verbose output
CURRENT_AUX_ACCOUNTS=(
    "GEQK8keXE4EZK5cAzNeoXgYrPPUNhH2iScKvT8y8sYhf"  # Aux-1
    "ECzvEE1YPpjL3KsZhbGed21JDbtPMHLNFG1RMpiRmyEV"  # Aux-2  
    "8PZCinZybav3fCq6SiE7j7k3RmsqCfM4iqG9GrrsKqPS"  # Aux-3
    "FaV7gjkGbzMRQ6xvq5FnYyt3K9FzCet3eGWJAu6jYees"  # Aux-4
    "HSpTGdRQhhFCTomHD7YcPTtumpze8q96UGbZEtCRrhcn"  # Aux-5
    "FkkXqq2V89nwsj9QsKnBirB9wkTJqJmphR429pUfBJf8"  # Aux-6
)

CLOSED_COUNT=0
FAILED_COUNT=0

for i in "${!CURRENT_AUX_ACCOUNTS[@]}"; do
    aux_num=$((i + 1))
    address="${CURRENT_AUX_ACCOUNTS[$i]}"
    
    echo "   🗑️  Closing Aux-$aux_num ($address)..."
    
    if spl-token close --address $address --owner $WALLET_FILE; then
        echo "   ✅ Successfully closed Aux-$aux_num"
        CLOSED_COUNT=$((CLOSED_COUNT + 1))
    else
        echo "   ❌ Failed to close Aux-$aux_num"
        FAILED_COUNT=$((FAILED_COUNT + 1))
    fi
    
    sleep 0.5
done

echo ""
echo "💰 SOL balance after:"
solana balance --keypair $WALLET_FILE

echo ""
echo "📊 Final CRD accounts:"
spl-token accounts --owner $WALLET_FILE | grep "$TOKEN_MINT"

echo ""
echo "📈 Summary:"
echo "   - Auxiliary accounts closed: $CLOSED_COUNT"
echo "   - Failed closures: $FAILED_COUNT"
echo "   - Total CRD transferred: 0.000002"
echo ""
echo "✨ Quick cleanup completed!"