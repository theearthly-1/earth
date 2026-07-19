# EARTH Token — Solana Smart Contract

**EARTH** is a Solana program (built with Anchor + Token-2022) that mints tokens to every iris-verified human on Earth, representing the planet's total economic value (~$500 trillion).

---

## Core Idea

Every verified human gets **66,000 EARTH** when they claim — and the same amount goes to the community treasury at the same time. No pre-mint. Tokens only exist when a real, verified person claims them. Supply is naturally capped by the human population — you can't fake a World ID iris scan.

The 66,000 allocation grows each year with Earth's real economic value (~3.5%/year, governance-adjustable), so people who join later get more EARTH than early adopters — because Earth is worth more.

---

## Treasury Milestone Unlocks

The community treasury is **locked by default**. It can only be distributed when humanity reaches scale:

| Milestone | Verified Humans | Treasury Release |
|-----------|----------------|------------------|
| Milestone 1 | 100,000,000 (100M) | 50% of treasury split equally to all registered humans |
| Milestone 2 | 500,000,000 (500M) | Remaining treasury split equally to all verified humans |

After each distribution the treasury keeps rebuilding from:
- New human registrations (66,000 per new person minted to treasury)
- Annual inflation (3.5% of total supply, split 50/50 treasury/inflation pool)

The more people who join, the faster the treasury builds toward the next milestone. Every new human benefits everyone already registered.

---

## How It Works

1. **Register** — World ID iris scan proves you're a real unique human
2. **Claim** — 66,000 EARTH minted to your vault, 66,000 to community treasury (free)
3. **Grow** — Annual revaluation grows the per-person allocation with Earth's value
4. **Earn** — Claim your equal share of annual inflation pool each year
5. **Milestone distributions** — When humanity hits 100M and 500M verified users, treasury distributes to everyone

---

## Key Design Decisions

- **No pre-mint** — zero tokens exist until a real human claims them
- **Dynamic allocation** — starts at 66,000, grows ~3.5%/year with Earth's value
- **Treasury locked until scale** — prevents early exploitation by small groups
- **Milestone eligibility** — only humans registered *before* a milestone is confirmed receive that distribution
- **Inflation split 50/50** — half to treasury, half claimable equally by all registered humans
- **World ID iris verification** — one person, one claim, no bots
- **AI and non-humans cannot own EARTH** — registration requires a physical iris scan; transfers require both parties to be registered humans; no exceptions, no governance override
- **1 human, 1 vote** — governance weighted by verified identity, not token holdings

---

## Instructions

| Instruction | Who | What |
|---|---|---|
| `initialize_mint` | Admin | Deploy the contract and create all token accounts |
| `register_human` | Oracle | Register a World ID verified human |
| `mint_birth_allocation` | Oracle | Mint allocation to human vault + treasury |
| `claim_vault` | Human | Claim your personal allocation |
| `mint_annual_inflation` | Anyone | Trigger annual inflation after 365 days |
| `claim_inflation_share` | Human | Claim your share of the inflation pool |
| `submit_annual_revaluation` | Admin | Update Earth value growth and per-human allocation |
| `confirm_milestone_1` | Admin + Governance | Lock in 50% treasury distribution at 100M humans |
| `confirm_milestone_2` | Admin + Governance | Lock in remaining treasury distribution at 500M humans |
| `claim_milestone_1_share` | Human | Claim your Milestone 1 treasury distribution |
| `claim_milestone_2_share` | Human | Claim your Milestone 2 treasury distribution |
| `create_proposal` | Human | Create a governance proposal |
| `cast_vote` | Human | Vote on a proposal (1 human = 1 vote) |
| `finalize_proposal` | Anyone | Finalize a proposal after voting ends |
| `emergency_freeze` | Admin | Instant freeze of all operations |
| `emergency_unfreeze` | Admin + Governance | Lift freeze after governance vote |

---

## Wallets

- **Primary admin:** `FndrmgjS9iZ7wgnj58fp49W3cMSc3XEfBYkYA8J4cTH3`
- **Backup admin:** `DHrSYVJbwTZr6xyS1YDzXTWJQqKfa5uA4NDJbLAg92Wb`

---

## Status

- [x] Contract written (Anchor + Token-2022)
- [ ] Devnet deployment and testing
- [ ] Oracle server (World ID → on-chain bridge)
- [ ] Claim website
- [ ] Mainnet deployment

---

## Tech Stack

- Solana + Anchor framework
- Token-2022 (SPL)
- World ID (Worldcoin) for iris verification
