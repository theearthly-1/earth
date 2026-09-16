# EARTH Claim Mini App

A World App Mini App: verify unique humanity via World ID, then claim a flat
1,000 EARTH allocation directly on World Chain.

## How it works

1. User connects their World App wallet via `MiniKit.walletAuth()`.
2. Clicking **Verify & Claim EARTH** generates a World ID 4.0 proof via
   `IDKit.request()` (`@worldcoin/idkit-core`), signalled with the connected
   wallet address so the proof can't be replayed against a different one.
3. That request needs a signed `rp_context` first — fetched from
   `/api/rp-signature`, a narrow server-only endpoint that signs only our own
   registered action with `RP_SIGNING_KEY`. This does NOT attest humanness or
   register anything; that's still the ZK proof plus the on-chain nullifier
   check in `EARTH.sol`'s `claim()`. It's the only thing that needs a private
   key, so it's the only thing that lives server-side.
4. The resulting proof is submitted directly on-chain via
   `MiniKit.sendTransaction()`, calling `EARTH.sol`'s `claim()` — which
   verifies the proof itself, directly, against World Chain's native World ID
   verifier. No oracle, no trusted relay, no backend that could lie about who
   verified.

## File structure

```
app/
  layout.tsx              Root layout, wraps the app in MiniKitProvider
  page.tsx                The claim page (steps 1-4 above)
  api/rp-signature/
    route.ts              Server-only: signs rp_context for our own action
providers/
  Providers.tsx            MiniKitProvider setup
lib/
  earth.ts                Contract address, ABI, on-chain arg conversions
  earth_abi.json           Real ABI, generated via `forge inspect EARTH abi`
```

## Prerequisites (one-time, before this app can work)

1. `EARTH.sol` deployed to World Chain (see `../contracts/EARTH.sol`), with
   its address set as `NEXT_PUBLIC_EARTH_CONTRACT_ADDRESS`.
2. A World ID Incognito Action registered in the
   [World Developer Portal](https://developer.worldcoin.org) matching
   `NEXT_PUBLIC_WORLD_ACTION_ID` — its hash must match the contract's
   immutable `CLAIM_ACTION_ID` exactly (see the contract's own NatSpec on how
   that hash is derived).
3. The deployed contract address added to the Developer Portal's Contract
   Entrypoints allowlist, or `MiniKit.sendTransaction` will be rejected.

## Environment variables

Copy `.env.local.example` to `.env.local` and fill in:

- `NEXT_PUBLIC_WORLDCHAIN_RPC_URL` — World Chain RPC endpoint.
- `NEXT_PUBLIC_EARTH_CONTRACT_ADDRESS` — the deployed `EARTH.sol` address.
- `NEXT_PUBLIC_WORLD_APP_ID` / `NEXT_PUBLIC_WORLD_ACTION_ID` /
  `NEXT_PUBLIC_WORLD_RP_ID` — from the World Developer Portal.
- `NEXT_PUBLIC_WORLD_ID_ENVIRONMENT` — `staging` or `production`. Controls
  which World ID credential preset is requested (device-level vs. real
  Orb-verified proof-of-human) — **must be `production` before real users
  claim**, `staging` has no real sybil resistance.
- `RP_SIGNING_KEY` — server-only, never exposed to the client. Signs
  `rp_context` for our own action only (see `app/api/rp-signature/route.ts`).

## Running

```bash
npm install
npm run dev
```

Note: `MiniKit.isInstalled()` only returns true when the page is actually
opened inside World App. Outside of it, verification is unavailable by
design — that's the whole point of proof-of-humanity.

## Known open items

- The exact conversions in `lib/earth.ts` (`rpIdToUint64`, `nonceUuidToUint256`)
  are inferred from bit-width alignment, not confirmed against an official
  worked example. First thing to check if real on-chain verification fails.
- Never tested inside actual World App yet — proven so far by TypeScript,
  `next build`, and static review only.
