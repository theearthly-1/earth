# EARTH

An ERC20 token on **World Chain** (an Ethereum L2), built around a simple
idea: every verified unique human can claim a flat, free allocation — proven
on-chain, with no trusted intermediary — alongside an open bonding-curve
market anyone can buy into or sell back to.

> **This project was originally built on Solana.** It was deliberately
> rebuilt on World Chain to get *direct on-chain* World ID verification,
> eliminating the trusted-oracle relay the Solana version required. See
> `contracts/EARTH.sol`'s own top-of-file NatSpec for the full reasoning.
> Solana-era history remains in this repo's git log; `programs/` (the old
> Anchor program) is not carried forward into current development.

## What it does

- **Claim**: any World ID-verified unique human (real Orb iris-scan
  verification, one claim per person ever, enforced by an on-chain ZK
  nullifier check against World Chain's native verifier) can claim a flat
  **1,000 EARTH**, free. No price input anywhere in the claim path — nothing
  for a caller to lie about.
- **Buy/sell**: a linear bonding curve (`price(s) = m·s + b`). Selling never
  burns — sold tokens go back into the contract's own balance and are resold
  to the next buyer before anything new is minted, so the full supply stays
  permanently visible and nothing is ever destroyed. Price moves naturally up
  on buys and down on sells; a 1.5% fee on every sell stays behind, slowly
  strengthening the vault's real backing over time.
- **Transparency**: `reserveStatus()` reports real ETH held vs. what it would
  cost to pay out everything currently circulating — an honest, checkable
  number anyone can read directly from the contract, not a claim to trust.
- **Builder allocation**: 5% of the 1-trillion-token cap, vested linearly to
  a multisig (6-month cliff, then 20 months) — never minted as a lump sum.
- **Admin**: minimal by construction. The only lever is swapping the World ID
  verifier address (staging → production before launch); it and every future
  admin-gated function are permanently disabled the moment `renounceAdmin()`
  is called. No pause, no freeze, no fund-sweep, ever.

## Structure

```
contracts/EARTH.sol       The contract. Solidity, World Chain.
test/EARTH.t.sol          Foundry test suite (41 tests, incl. a fuzz test)
miniapp/                  Next.js World App Mini App (claim UI, MiniKit + IDKit)
```

## Status

Contract: written, reviewed, 41/41 Foundry tests passing, live-proven with
real transactions on a local chain. Miniapp: rebuilt for World Chain/MiniKit,
typechecks and builds clean, adversarially reviewed. **Never yet deployed to
a public network or tested inside actual World App** — see `miniapp/README.md`
for the specific remaining steps.

## Local development

```bash
# Contract (requires Foundry)
forge build
forge test

# Mini app
cd miniapp
npm install
npm run dev
```
