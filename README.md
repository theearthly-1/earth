# EARTH — Solana Smart Contract

A Solana program that distributes 66,000 EARTHs to every World ID-verified human on Earth. One person, one allocation. No banks, no borders, no bullshit.

## What It Does

- **Mints 66,000 EARTH** to each verified human's personal vault
- **Mints 66,000 EARTH** to the community treasury per verified human
- **World ID iris verification** — one wallet per human, enforced on-chain
- **3.5% annual inflation** to the treasury, permissionless
- **1-human-1-vote governance** — proposals, quorum, voting, all on-chain
- **Death/heir mechanism** — unclaimed vaults can be transferred to heirs
- **Human-only ownership** — no corporate wallets, no bots

## Contract Details

| Parameter | Value |
|-----------|-------|
| Token Symbol | EARTH |
| Decimals | 6 |
| Per-human allocation | 66,000 EARTH |
| Annual inflation | 3.5% to treasury |
| Verification | World ID (iris biometric) |
| Blockchain | Solana |
| Framework | Anchor |

## Architecture

- `earth_lib_v2.rs` — Main program (1,200+ lines)
  - `initialize_mint` — Deploy the token and program state
  - `register_human` — World ID verification + vault creation
  - `claim_vault` — Human claims their 66,000 EARTH
  - `mint_inflation` — Permissionless 3.5% annual treasury mint
  - `create_proposal` / `cast_vote` / `execute_proposal` — Governance
  - `transfer_vault` — Heir mechanism for deceased holders
  - `emergency_freeze` / `emergency_unfreeze` — Admin safety controls

## Status

- [x] Contract written
- [x] Builds successfully (Solana Playground)
- [ ] Devnet deployment
- [ ] Mainnet deployment
- [ ] Claim website (World ID integration)

## Admin

- Primary admin: `FndrmgjS9iZ7wgnj58fp49W3cMSc3XEfBYkYA8J4cTH3`
- Backup admin: `DHrSYVJbwTZr6xyS1YDzXTWJQqKfa5uA4NDJbLAg92Wb`

## Vision

Every human being alive today gets 66,000 EARTHs — verified by iris scan, distributed by code, governed by the people holding it. No pre-mine, no VC allocation, no founder share. Just math and biology.

Built on Solana for speed and cost. Verified by World ID for humanity.
