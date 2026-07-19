# EARTH Token — Smart Contract

Every verified human on Earth gets an equal share of Earth's total value.

---

## What This Contract Does

- A verified human registers via World ID iris scan (oracle-signed)
- They call `claim_allocation` once — tokens are created at that moment and go straight to their wallet
- Equal amount goes to the community treasury at the same time
- Tokens are theirs. No restrictions after claiming — sell, trade, transfer freely
- No single wallet can hold more than **100 million EARTH**

---

## Every Instruction in the Contract

### Setup
| Instruction | Who | What |
|---|---|---|
| `initialize_mint` | Admin (once) | Creates the mint, treasury, inflation pool, and program state. Mints 1,000,000 EARTH to the founder wallet |
| `update_oracle` | Admin | Updates the World ID oracle signing key |

### Human Registration & Claiming
| Instruction | Who | What |
|---|---|---|
| `register_human` | Oracle | Registers a World ID verified human on-chain. One iris = one registration forever |
| `claim_allocation` | Registered human | Mints their EARTH allocation directly to their wallet. Tokens created at this moment. Once only. Enforces 100M wallet cap |

### Annual Inflation
| Instruction | Who | What |
|---|---|---|
| `submit_annual_revaluation` | Admin | Updates per-human allocation and inflation rate (~3.5%/yr). Permissioned, governance can reject within 30 days |
| `mint_annual_inflation` | Anyone | Triggers yearly inflation mint after 365 days. 50% → treasury, 50% → inflation pool |
| `claim_inflation_share` | Registered human | Claims their equal share of the inflation pool each epoch |

### Treasury Milestone Unlocks
The treasury is **locked** until humanity reaches scale. No one can spend it before then.

| Instruction | Who | What |
|---|---|---|
| `confirm_milestone_1` | Anyone | Permissionless. Contract checks `total_verified_humans ≥ 100M` itself. Locks in per-human distribution (50% of treasury ÷ total humans). Mints 1,000,000 EARTH to founder wallet |
| `claim_milestone_1_share` | Eligible human | Claims their share of the milestone 1 distribution. Must have registered before milestone was confirmed |
| `confirm_milestone_2` | Anyone | Permissionless. Contract checks `total_verified_humans ≥ 500M` itself. Distributes remaining treasury. Mints 10,000,000 EARTH to founder wallet |
| `claim_milestone_2_share` | Eligible human | Claims their share of the milestone 2 distribution |

### Governance
| Instruction | Who | What |
|---|---|---|
| `create_proposal` | Registered human | Creates a governance proposal |
| `cast_vote` | Registered human | Votes yes or no on an active proposal |
| `finalize_proposal` | Anyone | Closes voting after the period ends and records pass/fail |

---

## Governance Proposal Types
- `SystemChange` — general system changes
- `OracleUpdate` — change the World ID oracle
- `InfrastructureDeployment` — infrastructure decisions
- `AnnualRevaluation` — ratify or challenge the yearly annual revaluation
- `UpdateInflationRate` — adjust the 3.5% inflation rate
- `ConfirmMilestone1` — advisory vote only; milestones are confirmed permissionlessly by the contract itself
- `ConfirmMilestone2` — advisory vote only; milestones are confirmed permissionlessly by the contract itself

---

## Rules

- **Claim requires World ID iris scan** — one iris, one registration, one claim, forever
- **After claiming, tokens are yours** — sell, trade, transfer to anyone, no restrictions
- **No wallet can hold more than 100 million EARTH** — enforced on every mint
- **Treasury is locked until milestones** — 100M humans unlocks 50%, 500M unlocks the rest
- **No emergency freeze** — the system runs itself. Fork is the recovery mechanism
- **No admin override on milestone eligibility** — if you registered after the milestone, you don't get that distribution

---

## Token Economics

- **Genesis allocation:** 66,000 EARTH per verified human
- **Growth:** ~3.5%/year — late joiners get more because Earth is worth more
- **Supply:** minted on demand — 1,000,000 EARTH founder allocation at launch, everything else minted only when a human claims
- **Treasury:** receives equal amount to every human claim + 50% of annual inflation
- **Inflation pool:** 50% of annual inflation, split equally among all registered humans

---

## Wallets
- Primary admin: `FndrmgjS9iZ7wgnj58fp49W3cMSc3XEfBYkYA8J4cTH3`
- Backup admin: `DHrSYVJbwTZr6xyS1YDzXTWJQqKfa5uA4NDJbLAg92Wb`
