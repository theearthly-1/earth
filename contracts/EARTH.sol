// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {ERC20Capped} from "@openzeppelin/contracts/token/ERC20/extensions/ERC20Capped.sol";
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {Math} from "@openzeppelin/contracts/utils/math/Math.sol";

/// @notice World ID 4.0's native on-chain verifier, deployed natively on World Chain.
/// verify() is a `view` function: it cryptographically checks the zero-knowledge
/// proof and reverts if it's invalid, but it does NOT track nullifier reuse itself
/// — that bookkeeping is this contract's job (see `usedNullifiers`). Because this
/// interface declares `view`, every call site below compiles to a STATICCALL,
/// which makes it *impossible* at the EVM level for the verifier (or anything it
/// calls) to write state during that call — including re-entering EARTH. That is
/// what makes claim() safe from reentrancy through this external call without
/// needing any special-casing beyond the nonReentrant guard already in place.
///
///   Production (World Chain mainnet): 0x00000000009E00F9FE82CfeeBB4556686da094d7
///   Staging:                          0x703a6316c975DEabF30b637c155edD53e24657DB
interface IWorldIDVerifier {
    function verify(
        uint256 nullifier,
        uint256 action,
        uint64 rpId,
        uint256 nonce,
        uint256 signalHash,
        uint64 expiresAtMin,
        uint64 issuerSchemaId,
        uint256 credentialGenesisIssuedAtMin,
        uint256[5] calldata zeroKnowledgeProof
    ) external view;
}

/**
 * @title EARTH
 * @notice ERC20 token with a linear bonding-curve market. Selling never burns —
 * sold tokens go back into the vault's own balance and are resold to future
 * buyers before anything new is minted, so the full supply stays permanently
 * visible and nothing is ever destroyed (see `buy()`/`sell()`). Also has a
 * flat 1,000-token World ID claim (one per verified unique human, direct
 * on-chain verification — no oracle, no signer, no trusted intermediary, and
 * no price input to manipulate), and a 5% builder allocation that vests
 * linearly to a multisig after a cliff.
 *
 * @dev This is a faithful Solidity/World Chain port of an already-designed and
 * tested Solana/Anchor program (see the sibling `programs/earth/src/lib.rs` in
 * this repo). The economics, caps, fee, and lock durations below are carried
 * over exactly. Three things are deliberately NOT a literal byte-for-byte port,
 * each called out in detail at its point of use, because they were either the
 * explicit point of this port or because EVM's uint256 headroom makes a strictly
 * more precise implementation available at no extra risk:
 *
 *   1. World ID verification is direct and on-chain (IWorldIDVerifier), replacing
 *      the Solana version's trusted-oracle relay. This isn't an adaptation of
 *      convenience — eliminating that trusted intermediary is the entire reason
 *      this is being deployed on World Chain instead of Solana. See `claim()`.
 *
 *   2. The curve's buy/sell math is done in exact fixed-point (see `_curveCost`)
 *      rather than the reference implementation's floor-supply/ceiling-width
 *      rounding to whole tokens. The whole-token rounding scheme is what the
 *      Solana version needed to keep every intermediate product inside u128
 *      (max ~3.4e38); Solidity's uint256 (max ~1.16e77) has roughly 39 extra
 *      orders of magnitude of headroom, enough to do the identical algebra on
 *      raw 18-decimal amounts directly with only a single, final, sub-wei
 *      rounding step instead of a whole-token one. Worth confirming with the
 *      team: the reference implementation's own rounding-exploit test only
 *      checks that splitting a *purchase* into many tiny trades is never
 *      cheaper than buying it all at once — it does not check the sell side,
 *      and the floor/ceiling-to-whole-token scheme it uses can misprice a
 *      single trade by up to roughly one whole token's marginal price in
 *      *either* direction depending on the exact fractional alignment of the
 *      starting supply, not only in the protocol's favor. This port's
 *      approach removes that ambiguity by construction (see `_curveCost`).
 *
 *   3. Token amounts (buy/sell) are raw 18-decimal units throughout, the
 *      normal ERC20/Solidity convention, rather than whole-token counts scaled
 *      up internally. The bonding-curve *price* is still defined in terms of
 *      whole tokens per the agreed spec (m, b are wei-per-whole-token, fixed-
 *      point scaled by CURVE_PRICE_SCALE — see `m`/`b`), but the amounts users
 *      pass in and receive are ordinary token-wei. The claim itself (see
 *      `claim()`) mints a flat CLAIM_AMOUNT_TOKENS regardless of curve price —
 *      there is no price input anywhere in the claim path to get wrong.
 *
 * Every other constant (cap, builder allocation %, cliff/vesting durations,
 * sell fee, claim lock duration) is copied verbatim from the tested reference.
 */
contract EARTH is ERC20Capped, ReentrancyGuard, Ownable {
    // ========================================================================
    // ERRORS
    // ========================================================================

    error ZeroAmount();
    error ZeroAddress();
    error InvalidCurveParams();
    error SlippageExceeded(uint256 actual, uint256 limit);
    error InsufficientPayment(uint256 sent, uint256 required);
    error InsufficientReserves(uint256 available, uint256 required);
    error InsufficientBalance(uint256 available, uint256 requested);
    error TokensLocked(uint256 sellable, uint256 requested);
    error ExceedsSupply();
    error ExceedsCap();
    error AlreadyClaimed();
    error TransferFailed();
    error NotVestingRecipient();
    error NothingToRelease();

    // ========================================================================
    // EVENTS
    // ========================================================================

    event Buy(address indexed buyer, uint256 tokenAmount, uint256 costWei);
    event Sell(address indexed seller, uint256 tokenAmount, uint256 netProceedsWei, uint256 feeWei);
    event Claim(address indexed claimant, uint256 indexed nullifier, uint256 tokenAmount);
    event VestingReleased(uint256 amountWei, uint256 totalReleasedWei);
    event WorldIdVerifierUpdated(address indexed newVerifier);

    // ========================================================================
    // CONSTANTS
    // ========================================================================

    /// @dev 1 whole token = 1e18 token-wei, the standard ERC20/Solidity convention
    /// (the reference Solana version used 9 decimals to match SOL's own; this port
    /// deliberately uses 18 to match ETH/Solidity convention per the agreed spec —
    /// confirm this is the intended decimals choice, see open questions).
    uint256 public constant TOKEN_UNIT = 1e18;

    /// @notice Hard cap: 1 trillion whole EARTH tokens, enforced on every mint via
    /// ERC20Capped (see constructor) in addition to the curve/claim/vesting-specific
    /// accounting below. Since selling no longer burns (see `sell()`), this is a
    /// genuinely permanent ceiling on cumulative minting activity — unlike an
    /// earlier design, no amount of selling ever frees up room under it again.
    uint256 public constant MAX_SUPPLY_TOKENS = 1_000_000_000_000;
    uint256 public constant MAX_SUPPLY_WEI = MAX_SUPPLY_TOKENS * TOKEN_UNIT; // 1e30

    /// @notice Builder allocation: 5% of the supply cap, vested — never minted as a
    /// lump sum. 6-month cliff, then linear over the following 20 months. Durations
    /// are copied verbatim (in seconds) from the tested reference implementation's
    /// BUILDER_CLIFF_SECONDS / BUILDER_VESTING_SECONDS constants: 180 days and 600
    /// days respectively (i.e. 30-day months) — confirm this approximation of
    /// "month" is what's intended; see open questions.
    uint256 public constant BUILDER_VESTING_ALLOCATION_WEI = (MAX_SUPPLY_WEI * 5) / 100; // 5e28
    uint256 public constant BUILDER_CLIFF_SECONDS = 15_552_000; // 180 days
    uint256 public constant BUILDER_VESTING_SECONDS = 51_840_000; // 600 days, after the cliff

    /// @notice Sell-back fee, taken before the tokens are returned to the vault's
    /// own balance: 1.5% (150 bps).
    uint256 public constant SELL_FEE_BPS = 150;
    uint256 public constant BPS_DENOMINATOR = 10_000;

    /// @notice Tokens minted via a World ID claim are locked 30 days before they can
    /// be sold back to the curve (they can still be held or transferred — see the
    /// note on `_lockedBalanceOf` about what this lock does and doesn't restrict).
    uint256 public constant CLAIM_LOCK_SECONDS = 2_592_000; // 30 days

    /// @notice Fixed-point scale for `m`/`b` (see below), matching the reference
    /// implementation's CURVE_PRICE_SCALE convention: lets the deployer express
    /// fractional wei-per-whole-token prices as integers.
    uint256 public constant CURVE_PRICE_SCALE = 1_000_000; // 1e6

    /// @dev Constructor-enforced ceiling on `m`. Not present in the reference
    /// implementation (which instead achieved safety by rounding to whole tokens
    /// before the quadratic multiply — see the contract-level NatSpec). This port
    /// keeps full raw-unit precision instead, which requires an explicit bound to
    /// make the overflow-safety argument airtight; see the proof in `_curveCost`.
    /// Realistic `m` for the agreed $0.001 -> $2 price band across the 1-trillion
    /// token cap is on the order of ~6.7e8, about 7-8 orders of magnitude below
    /// this ceiling — this is a safety backstop, not an economic constraint.
    uint256 public constant MAX_CURVE_M = 1e16;
    uint256 public constant MAX_CURVE_B = 1e30;

    /// @dev Precomputed divisors for the exact-fixed-point curve integral, see
    /// `_curveCost`. QUAD_DIVISOR = 2 * TOKEN_UNIT^2 * CURVE_PRICE_SCALE.
    uint256 private constant QUAD_DIVISOR = 2 * TOKEN_UNIT * TOKEN_UNIT * CURVE_PRICE_SCALE; // 2e42
    uint256 private constant LIN_DIVISOR = TOKEN_UNIT * CURVE_PRICE_SCALE; // 1e24

    /// @notice Flat amount of EARTH every verified unique human receives on
    /// claim, regardless of curve price or supply. There is no price input
    /// anywhere in the claim path (see `claim()`) — removing it removes the
    /// entire class of "caller lies about the price" risk instead of just
    /// bounding it.
    uint256 public constant CLAIM_AMOUNT_TOKENS = 1_000;
    uint256 public constant CLAIM_AMOUNT_RAW = CLAIM_AMOUNT_TOKENS * TOKEN_UNIT;

    // ========================================================================
    // IMMUTABLES
    // ========================================================================

    /// @notice Linear bonding curve: price(s) = m*s + b, in wei per whole token,
    /// where s is circulating supply in whole tokens. Both `m` and `b` are
    /// fixed-point integers scaled by CURVE_PRICE_SCALE (e.g. a real slope of
    /// 667 wei/token^2 is passed as `m = 667_000_000`). Set once at deploy time
    /// and never changed — the curve's shape is not an admin lever.
    uint256 public immutable m;
    uint256 public immutable b;

    /// @notice Fixed World ID action identifier this contract's claim is bound to.
    /// Deliberately NOT a caller-supplied parameter: the "one claim per person"
    /// guarantee only holds if every claimant proves against the *same* action,
    /// since World ID nullifiers are unique per (person, action) — letting a
    /// caller pick their own action would let one person claim once per action.
    /// Deploy-time constant, immutable, no admin lever to change it later.
    uint256 public immutable CLAIM_ACTION_ID;

    /// @notice Multisig that receives the vested builder allocation. MUST be a
    /// genuine multisig (e.g. a Gnosis Safe) in practice — the contract cannot
    /// enforce that on-chain, but nothing here treats it as anything other than
    /// an opaque address, and it is immutable: not admin-changeable post-deploy,
    /// so admin can never redirect the builder allocation to itself.
    address public immutable builderVestingMultisig;

    /// @notice Vesting cliff/schedule reference point, set to deploy time.
    uint256 public immutable vestingStart;

    // ========================================================================
    // MUTABLE STATE
    // ========================================================================

    /// @notice World Chain's native World ID verifier proxy. Admin-settable
    /// (see `setWorldIdVerifier`) so a single deployment can start against the
    /// staging verifier and be flipped to the production one before launch,
    /// without a redeploy — but this lever, like every admin lever, is
    /// permanently frozen the moment `renounceAdmin()` is called. This mirrors
    /// the reference implementation's `update_oracle`-until-renounce pattern
    /// exactly, just pointed at a verifier contract instead of a trusted oracle.
    IWorldIDVerifier public worldIdVerifier;

    /// @notice One-time-use World ID nullifiers. This mapping — not the verifier,
    /// which is a stateless `view` check — is what makes "one claim per person"
    /// an on-chain guarantee rather than merely a proof property.
    mapping(uint256 nullifier => bool used) public usedNullifiers;

    /// @notice Defense-in-depth alongside `usedNullifiers`: pins one claim per
    /// address as well as per nullifier. Not present in the reference
    /// implementation (which relies solely on the nullifier-seeded PDA); added
    /// here because it's essentially free and closes off any scenario where a
    /// single person could obtain more than one valid nullifier against
    /// CLAIM_ACTION_ID (e.g. across a credential rotation) — confirm this
    /// matches intent, see open questions.
    mapping(address account => bool claimed) public hasClaimed;

    /// @notice How many token-wei of `account`'s balance are still under the
    /// 30-day claim lock, and until when. See `_lockedBalanceOf`.
    mapping(address account => uint256 amountWei) public claimLockedAmount;
    mapping(address account => uint256 unlockTimestamp) public claimUnlockTime;

    /// @notice Cumulative wei released from the builder vesting allocation so
    /// far. Tracked separately from `totalSupply()` because totalSupply() mixes
    /// several sources (buys, claims, vesting releases) — there's no way to
    /// isolate "how much came from vesting" without its own counter.
    uint256 public vestingReleasedWei;

    // ========================================================================
    // CONSTRUCTOR
    // ========================================================================

    /// @param curveM Curve slope, wei-per-whole-token^2, scaled by CURVE_PRICE_SCALE.
    /// @param curveB Curve intercept (price at zero supply), wei-per-whole-token,
    /// scaled by CURVE_PRICE_SCALE. Must be > 0 — a zero intercept would make the
    /// first tokens off the curve free, which the reference implementation does
    /// not explicitly guard against but this port does; confirm this is desired,
    /// see open questions.
    /// @param worldIdVerifierAddress World Chain's native World ID verifier proxy
    /// (production or staging — see IWorldIDVerifier NatSpec for both addresses).
    /// @param claimActionId Fixed World ID action ID this deployment's claim() is
    /// bound to. Immutable; see `CLAIM_ACTION_ID`.
    /// @param builderVestingMultisig_ MUST be a multisig address in practice (e.g.
    /// a Gnosis Safe) — see `builderVestingMultisig`.
    /// @param admin_ Setup-only admin; see `setWorldIdVerifier` / `renounceAdmin`.
    constructor(
        uint256 curveM,
        uint256 curveB,
        address worldIdVerifierAddress,
        uint256 claimActionId,
        address builderVestingMultisig_,
        address admin_
    ) ERC20("EARTH", "EARTH") ERC20Capped(MAX_SUPPLY_WEI) Ownable(admin_) {
        if (curveM == 0 || curveM > MAX_CURVE_M) revert InvalidCurveParams();
        if (curveB == 0 || curveB > MAX_CURVE_B) revert InvalidCurveParams();
        if (worldIdVerifierAddress == address(0)) revert ZeroAddress();
        if (builderVestingMultisig_ == address(0)) revert ZeroAddress();

        m = curveM;
        b = curveB;
        worldIdVerifier = IWorldIDVerifier(worldIdVerifierAddress);
        CLAIM_ACTION_ID = claimActionId;
        builderVestingMultisig = builderVestingMultisig_;
        vestingStart = block.timestamp;
    }

    /// @dev No `receive`/`fallback`. All ETH must arrive through `buy()`, which is
    /// the only path that has any accounting for it. Accepting stray transfers
    /// here would silently inflate the curve's reserves with nothing minted
    /// against them and no way to ever return them (there is deliberately no
    /// admin sweep function — see the contract-level admin-scope discussion) —
    /// rejecting them outright is safer than accepting mystery ETH.

    // ========================================================================
    // BONDING CURVE — buy (from vault stock, then mint) / sell (to vault stock)
    // ========================================================================

    /// @notice Buys `tokenAmount` (token-wei, 18 decimals) of EARTH. Fills the
    /// order from whatever EARTH the vault itself is currently holding (tokens
    /// previously sold back — see `sell()`) BEFORE minting anything new, so
    /// sold-back supply is genuinely resold rather than sitting inert forever.
    /// Pays the exact integral cost in ETH, capped by `maxWeiCost` (slippage
    /// protection). Any msg.value above the actual cost is refunded.
    ///
    /// @dev Priced off `_circulatingSupply()`, the same basis `sell()` uses —
    /// the price moves up and down naturally with ordinary buying and selling,
    /// exactly like before this contract stopped burning on sell. Recycling
    /// vault stock instead of minting doesn't change the price at all; it only
    /// changes whether new tokens get created to satisfy the order. What
    /// changes over time is the vault's real ETH backing, which trends
    /// upward through ordinary trading because every sell keeps 1.5% behind
    /// (see `reserveStatus()`) — not the curve price itself, which is free to
    /// rise and fall with real supply and demand.
    function buy(uint256 tokenAmount, uint256 maxWeiCost) external payable nonReentrant {
        if (tokenAmount == 0) revert ZeroAmount();
        if (tokenAmount > MAX_SUPPLY_WEI) revert ExceedsSupply();

        uint256 rawSupply0 = totalSupply();
        uint256 vaultStock = balanceOf(address(this));
        uint256 circulating0 = rawSupply0 - vaultStock;
        // Keeps the pricing range within the same MAX_SUPPLY_WEI bound
        // `_curveCost`'s overflow proof assumes, regardless of how much of
        // tokenAmount ends up filled from vault stock rather than freshly
        // minted (see the proof in `_curveCost`) — this exact condition could
        // never trigger under the old mint-only buy(), since a full mint of
        // tokenAmount always implied this already; it only becomes reachable
        // now that a buy can request more than fits under the cap while still
        // being partly stock-filled.
        if (circulating0 + tokenAmount > MAX_SUPPLY_WEI) revert ExceedsSupply();

        uint256 fromStock = tokenAmount > vaultStock ? vaultStock : tokenAmount;
        uint256 toMint = tokenAmount - fromStock;

        // Only the freshly-minted portion needs cap/vesting-reserve room —
        // stock already exists and was counted against the cap when it was
        // first minted, long before this buy. This check is deliberately
        // against raw totalSupply() (how many tokens have EVER been created),
        // not circulating supply (how many are currently out and about) —
        // the cap is about permanent creation, not current circulation.
        if (toMint > 0) {
            _requireCurveAndClaimRoom(rawSupply0 + toMint);
        }

        uint256 cost = _curveCost(circulating0, circulating0 + tokenAmount, true); // round up: never undercharge
        if (cost > maxWeiCost) revert SlippageExceeded(cost, maxWeiCost);
        if (msg.value < cost) revert InsufficientPayment(msg.value, cost);

        // Effects before the one external call (the refund) below — fully
        // committed first, so even though `nonReentrant` already blocks
        // re-entry, a reentrant call here (were the guard ever removed) would
        // observe fully-updated state and could not double-spend.
        if (toMint > 0) _mint(msg.sender, toMint);
        if (fromStock > 0) _transfer(address(this), msg.sender, fromStock);

        uint256 refund = msg.value - cost;
        if (refund > 0) {
            (bool ok, ) = msg.sender.call{value: refund}("");
            if (!ok) revert TransferFailed();
        }

        emit Buy(msg.sender, tokenAmount, cost);
    }

    /// @notice Sells `tokenAmount` (token-wei) back to the vault: pays out the
    /// curve's integral value minus SELL_FEE_BPS, straight from this contract's
    /// own ETH balance. The tokens are NOT burned — they're transferred to this
    /// contract's own balance, where a future `buy()` resells them before ever
    /// minting anything new. Nothing is ever destroyed; `totalSupply()` never
    /// decreases from a sell.
    ///
    /// @dev Pricing basis: uses `_circulatingSupply()` (totalSupply() minus
    /// whatever this contract already holds), NOT raw `totalSupply()`. This
    /// matters — since selling no longer reduces totalSupply() itself, using
    /// raw totalSupply() here would mean every seller prices off the same
    /// unmoving "top" of the curve, letting repeated sells each get paid as if
    /// they were the newest, most expensive slice, draining the vault far
    /// faster than intended. `_circulatingSupply()` correctly shrinks as
    /// tokens come back to the vault, exactly mirroring how raw totalSupply()
    /// used to behave under the old burn-based design — so each successive
    /// seller still prices correctly below the last, and everything already
    /// proven about vault solvency (see `reserveStatus()`) still holds.
    ///
    /// @dev Reentrancy safety: this function follows strict checks-effects-
    /// interactions ordering — every check (balance, lock, slippage, reserve
    /// sufficiency) and every state mutation (the transfer to this contract)
    /// happens BEFORE the single external call (the ETH payout) at the very
    /// end. If the recipient were a malicious contract that tried to re-enter
    /// `sell()` (or `buy()`) from its receive hook, it would find
    /// `nonReentrant` blocking it outright; and even in a hypothetical world
    /// without that guard, it would see its own balance already reduced by
    /// the transfer and the vault's ETH already committed to this payout, so
    /// there is nothing left to double-spend. If the vault doesn't currently
    /// hold enough ETH (heavy selling having outpaced buying), this reverts
    /// cleanly before any state changes — nothing is lost, the caller's
    /// tokens are untouched, and they can retry once more buying has refilled
    /// the vault.
    function sell(uint256 tokenAmount, uint256 minWeiProceeds) external nonReentrant {
        if (tokenAmount == 0) revert ZeroAmount();

        uint256 balance = balanceOf(msg.sender);
        if (balance < tokenAmount) revert InsufficientBalance(balance, tokenAmount);

        // A World ID claim locks its minted tokens for 30 days: the seller may
        // only sell down to (balance - stillLocked), never below it. Pure market
        // buyers who never claimed have `claimUnlockTime == 0`, so the lock check
        // below is always trivially satisfied for them.
        //
        // `locked` is a flat amount, not tied to specific tokens (ERC20 balances
        // are fungible, so there's no such thing as "the specific claimed
        // tokens"). If the caller transferred tokens out after claiming, their
        // current balance can legitimately end up below `locked` — treat that
        // as fully locked (sellable = 0) rather than underflowing, since we
        // have no way to know whether what remains is "the claimed portion" or
        // not, and 0 is the safe, conservative answer either way.
        uint256 locked = _lockedBalanceOf(msg.sender);
        uint256 sellable = balance > locked ? balance - locked : 0;
        if (tokenAmount > sellable) revert TokensLocked(sellable, tokenAmount);

        uint256 s1 = _circulatingSupply();
        if (tokenAmount > s1) revert ExceedsSupply();
        uint256 s0 = s1 - tokenAmount;

        uint256 grossProceeds = _curveCost(s0, s1, false); // round down: never overpay
        uint256 fee = (grossProceeds * SELL_FEE_BPS) / BPS_DENOMINATOR;
        uint256 netProceeds = grossProceeds - fee;
        if (netProceeds < minWeiProceeds) revert SlippageExceeded(netProceeds, minWeiProceeds);
        if (address(this).balance < netProceeds) {
            revert InsufficientReserves(address(this).balance, netProceeds);
        }

        _transfer(msg.sender, address(this), tokenAmount);

        // Dust-sized sells can legitimately floor to 0 net wei (e.g. selling a
        // handful of raw token-wei at a low spot price) — that's not an error,
        // just nothing to pay out, so skip the external call entirely rather
        // than making a pointless 0-value call to msg.sender (mirrors the
        // `refund > 0` guard in `buy()`).
        if (netProceeds > 0) {
            (bool ok, ) = msg.sender.call{value: netProceeds}("");
            if (!ok) revert TransferFailed();
        }

        emit Sell(msg.sender, tokenAmount, netProceeds, fee);
    }

    // ========================================================================
    // WORLD ID CLAIM
    // ========================================================================

    /// @notice Claims a flat CLAIM_AMOUNT_TOKENS (1,000) EARTH, gated by a World
    /// ID uniqueness proof verified directly on-chain — no oracle, no signing
    /// server, no trusted intermediary, and no price input of any kind. One
    /// claim per unique human, enforced by `usedNullifiers` (World ID's
    /// `verify()` is a stateless view check; it does not itself prevent
    /// nullifier reuse).
    ///
    /// @dev `action` (fixed to CLAIM_ACTION_ID) and `signalHash` are NOT accepted
    /// as caller parameters, unlike every other verify() argument, which are
    /// pure pass-through: `action` must be fixed or a person could mint one
    /// nullifier per distinct action value and claim repeatedly (see
    /// CLAIM_ACTION_ID), and `signalHash` must be derived from msg.sender by
    /// this contract rather than trusted from the caller, or a valid proof
    /// generated for one wallet could be replayed to mint into a different one.
    /// The claimant's World ID client MUST use this same wallet address as the
    /// signal when generating the proof off-chain, or verification will fail.
    ///
    /// @param nullifier World ID nullifier hash for (this person, CLAIM_ACTION_ID).
    /// @param rpId, nonce, expiresAtMin, issuerSchemaId, credentialGenesisIssuedAtMin,
    /// zeroKnowledgeProof: pass-through World ID 4.0 proof parameters, forwarded
    /// to `worldIdVerifier.verify()` unmodified. That call is expected to enforce
    /// proof validity, expiry, and credential-genesis constraints itself; this
    /// contract does not duplicate that validation (see open questions).
    function claim(
        uint256 nullifier,
        uint64 rpId,
        uint256 nonce,
        uint64 expiresAtMin,
        uint64 issuerSchemaId,
        uint256 credentialGenesisIssuedAtMin,
        uint256[5] calldata zeroKnowledgeProof
    ) external nonReentrant {
        if (usedNullifiers[nullifier]) revert AlreadyClaimed();
        if (hasClaimed[msg.sender]) revert AlreadyClaimed();

        uint256 signalHash = _hashToField(abi.encodePacked(msg.sender));

        // Reverts internally (no boolean return) on any invalid, expired, or
        // mis-bound proof. Compiles to a STATICCALL (see IWorldIDVerifier
        // NatSpec) so this call cannot itself be a reentrancy vector.
        worldIdVerifier.verify(
            nullifier,
            CLAIM_ACTION_ID,
            rpId,
            nonce,
            signalHash,
            expiresAtMin,
            issuerSchemaId,
            credentialGenesisIssuedAtMin,
            zeroKnowledgeProof
        );

        // Effects: burn the nullifier and the per-address claim flag immediately
        // after a successful verification. A revert anywhere below rolls this
        // back too (atomic transaction), so this ordering isn't load-bearing for
        // correctness, but it's the right habit and costs nothing.
        usedNullifiers[nullifier] = true;
        hasClaimed[msg.sender] = true;

        uint256 s1 = totalSupply() + CLAIM_AMOUNT_RAW;
        _requireCurveAndClaimRoom(s1);

        _mint(msg.sender, CLAIM_AMOUNT_RAW);

        claimLockedAmount[msg.sender] = CLAIM_AMOUNT_RAW;
        claimUnlockTime[msg.sender] = block.timestamp + CLAIM_LOCK_SECONDS;

        emit Claim(msg.sender, nullifier, CLAIM_AMOUNT_RAW);
    }

    // ========================================================================
    // BUILDER VESTING
    // ========================================================================

    /// @notice Mints whatever has newly vested since the last release, to the
    /// builder multisig. Callable only by that multisig. Reverts if nothing new
    /// has vested (including the entire pre-cliff period, where 0 has vested).
    function releaseVesting() external nonReentrant {
        if (msg.sender != builderVestingMultisig) revert NotVestingRecipient();

        uint256 vested = _vestedAmount(block.timestamp);
        uint256 releasable = vested - vestingReleasedWei; // vested is monotonic, never < vestingReleasedWei
        if (releasable == 0) revert NothingToRelease();

        vestingReleasedWei += releasable;
        _mint(builderVestingMultisig, releasable);

        emit VestingReleased(releasable, vestingReleasedWei);
    }

    /// @notice Total builder-allocation wei vested as of `timestamp`: 0 before the
    /// 6-month cliff, then linear over the following 20 months, capped at the
    /// full allocation. Both durations are exact seconds copied from the tested
    /// reference implementation (180 days / 600 days — see the constants above).
    function _vestedAmount(uint256 timestamp) internal view returns (uint256) {
        uint256 cliffEnd = vestingStart + BUILDER_CLIFF_SECONDS;
        if (timestamp < cliffEnd) {
            return 0;
        }
        uint256 fullyVestedAt = cliffEnd + BUILDER_VESTING_SECONDS;
        if (timestamp >= fullyVestedAt) {
            return BUILDER_VESTING_ALLOCATION_WEI;
        }
        uint256 elapsed = timestamp - cliffEnd;
        return (BUILDER_VESTING_ALLOCATION_WEI * elapsed) / BUILDER_VESTING_SECONDS;
    }

    // ========================================================================
    // ADMIN — setup-only, minimal by construction. There is deliberately no
    // pause switch, freeze function, fund-sweep, or upgrade path anywhere in
    // this contract, and none of what follows survives `renounceAdmin()`.
    // ========================================================================

    /// @notice Swaps the World ID verifier contract (e.g. staging -> production
    /// before launch, without a redeploy). See `worldIdVerifier` NatSpec for why
    /// this is admin-settable rather than immutable, and why that's still safe:
    /// this lever is permanently gone the instant `renounceAdmin()` is called,
    /// exactly like every other admin-gated function.
    function setWorldIdVerifier(address newVerifier) external onlyOwner {
        if (newVerifier == address(0)) revert ZeroAddress();
        worldIdVerifier = IWorldIDVerifier(newVerifier);
        emit WorldIdVerifierUpdated(newVerifier);
    }

    /// @notice One-way, permanent. After this call, `setWorldIdVerifier` (and any
    /// admin-gated function ever added in a future version of this contract) is
    /// disabled forever, for everyone, including the original admin. There is no
    /// un-renounce and no backdoor — this is the whole point of shipping on World
    /// Chain instead of depending on a persistent trusted operator. Implemented
    /// via OpenZeppelin's `Ownable.renounceOwnership`, which sets the owner to
    /// `address(0)` — a value no transaction can ever originate from — making
    /// every `onlyOwner` function unconditionally and permanently unreachable.
    function renounceAdmin() external onlyOwner {
        renounceOwnership();
    }

    // ========================================================================
    // CURVE MATH
    // ========================================================================

    /// @dev Exact integral of price(s) = m*s + b over the raw-token-wei range
    /// [s0Raw, s1Raw], i.e. the wei cost (buy) or proceeds (sell) for that
    /// range: m/2*(S1^2-S0^2) + b*(S1-S0), where S = sRaw / TOKEN_UNIT is the
    /// whole-token-equivalent (real-valued) supply.
    ///
    /// Substituting S1^2-S0^2 = (S1-S0)(S1+S0) and clearing every denominator
    /// into a single division per term (rather than converting s0Raw/s1Raw to
    /// whole tokens up front via floor/ceiling, as an earlier draft of this port
    /// did) gives:
    ///
    ///   quadratic part = m * dsRaw * (s0Raw+s1Raw) / (2 * TOKEN_UNIT^2 * CURVE_PRICE_SCALE)
    ///   linear part    = b * dsRaw / (TOKEN_UNIT * CURVE_PRICE_SCALE)
    ///
    /// with exactly one rounding step, at the very end of each term, instead of
    /// rounding the supply range out to whole tokens first. That matters: this
    /// port supports fractional (sub-whole-token) trade sizes since amounts are
    /// ordinary token-wei, and rounding the *supply range* out to the nearest
    /// whole token before pricing can mis-price a single trade by up to roughly
    /// one whole token's marginal price — see the contract-level NatSpec for the
    /// full comparison. Rounding only the *final wei amount* bounds the error to
    /// a single wei, in a known direction controlled by `roundUp`: `true` for
    /// buy (never charge less than the exact cost) and `false` for sell (never
    /// pay out more than the exact proceeds). Both directions favor the
    /// protocol by at most 1 wei per call, which is not economically
    /// exploitable even by splitting a trade into the smallest possible
    /// increments (each increment still rounds against the trader, not for
    /// them).
    ///
    /// === Overflow proof ===
    /// `dsRaw = s1Raw - s0Raw` and `s0Raw + s1Raw` are each bounded by
    /// MAX_SUPPLY_WEI (1e30) and 2*MAX_SUPPLY_WEI (2e30) respectively, since
    /// supply can never exceed the 1-trillion-token cap (enforced both by
    /// `_requireCurveAndClaimRoom` before every mint and by ERC20Capped as a
    /// hard backstop). Their product — the single widest value computed before
    /// multiplying by `m` — is therefore at most 1e30 * 2e30 = 2e60, itself a
    /// tiny fraction of uint256's ~1.1579e77 ceiling (roughly 17 orders of
    /// magnitude of headroom on its own). Multiplying that by `m`, which the
    /// constructor caps at MAX_CURVE_M = 1e16, bounds the quadratic numerator
    /// at 2e76 — under uint256's max with a further ~17% of the entire range to
    /// spare — even for the single worst-case transaction physically possible
    /// on this contract: buying the entire supply from 0 straight to the
    /// 1-trillion-token cap in one call. The linear numerator (`b * dsRaw`,
    /// with `b` capped at MAX_CURVE_B = 1e30) is bounded at 1e60, dwarfed by
    /// the same ceiling with even more room to spare. Both intermediate
    /// products individually fit; their sum (the final returned cost) is
    /// smaller still since each divisor exceeds 1. No step in this function
    /// can overflow for any curveM/curveB the constructor accepts.
    function _curveCost(uint256 s0Raw, uint256 s1Raw, bool roundUp) internal view returns (uint256) {
        uint256 dsRaw = s1Raw - s0Raw; // caller guarantees s1Raw >= s0Raw
        if (dsRaw == 0) return 0;
        uint256 sumRaw = s0Raw + s1Raw;

        uint256 quadNumerator = m * (dsRaw * sumRaw);
        uint256 quadPart = roundUp
            ? Math.ceilDiv(quadNumerator, QUAD_DIVISOR)
            : quadNumerator / QUAD_DIVISOR;

        uint256 linNumerator = b * dsRaw;
        uint256 linPart = roundUp
            ? Math.ceilDiv(linNumerator, LIN_DIVISOR)
            : linNumerator / LIN_DIVISOR;

        return quadPart + linPart;
    }

    /// @dev Ensures buy()/claim() minting never eats into the tokens permanently
    /// reserved for the builder vesting schedule. The vesting schedule is
    /// entitled to BUILDER_VESTING_ALLOCATION_WEI regardless of how much
    /// curve/claim activity happens; `reserve` is the portion of that
    /// allocation not yet vested-and-minted, which must always remain
    /// available headroom under the hard cap. `s1` is the resulting
    /// totalSupply() *after* the mint being checked.
    function _requireCurveAndClaimRoom(uint256 s1) internal view {
        uint256 reserve = BUILDER_VESTING_ALLOCATION_WEI - vestingReleasedWei;
        if (s1 + reserve > MAX_SUPPLY_WEI) revert ExceedsCap();
    }

    /// @dev How much EARTH is actually out in people's hands right now, as
    /// opposed to `totalSupply()`, which also counts tokens sold back to the
    /// vault (never burned, see `sell()`) and still sitting in this contract's
    /// own balance. This is what `sell()`, `previewSellProceeds()`, and
    /// `reserveStatus()` price/measure against — those tokens aren't at risk
    /// of being redeemed twice, so they shouldn't count as still circulating.
    /// Derived entirely from standard ERC20 state (totalSupply, one balance
    /// lookup) — no separate bookkeeping variable to trust.
    function _circulatingSupply() internal view returns (uint256) {
        return totalSupply() - balanceOf(address(this));
    }

    /// @dev Portion of `account`'s balance still under the 30-day World ID claim
    /// lock, or 0 once CLAIM_LOCK_SECONDS has elapsed since their claim (or if
    /// they never claimed at all — `claimUnlockTime` defaults to 0, and
    /// `block.timestamp >= 0` is always true).
    ///
    /// This is a balance-floor check performed only inside `sell()`, exactly as
    /// specified: it stops the claiming wallet from selling locked tokens back
    /// to the curve, but it does NOT stop that wallet from transferring locked
    /// tokens to a different address first, which could then sell them
    /// immediately with no lock recorded against it. Plain ERC20 transfers are
    /// not hooked here to enforce the lock more strictly. This matches the
    /// reference implementation's identical design (its lock is likewise only
    /// checked inside `sell_to_curve`) rather than being a gap introduced by
    /// this port — flagged here for explicit human sign-off; see open
    /// questions.
    function _lockedBalanceOf(address account) internal view returns (uint256) {
        if (block.timestamp >= claimUnlockTime[account]) {
            return 0;
        }
        return claimLockedAmount[account];
    }

    /// @dev World ID signal-hashing convention: keccak256 the ABI-encoded signal,
    /// then right-shift by 8 bits so the result fits the SNARK-friendly field
    /// used by the proof system. This must exactly match whatever hash-to-field
    /// routine the World ID client SDK uses when the claimant's wallet generates
    /// its proof off-chain (signal = the claimant's own address) — verify this
    /// against World ID 4.0's current official documentation before deploying;
    /// see open questions. A mismatch here fails safe (100% of claims simply
    /// revert) rather than open a security hole, but it would still mean claims
    /// don't work at all.
    function _hashToField(bytes memory value) internal pure returns (uint256) {
        return uint256(keccak256(value)) >> 8;
    }

    // ========================================================================
    // VIEW HELPERS (no side effects; convenience for front-ends/integrators)
    // ========================================================================

    /// @notice Instantaneous marginal price at the current circulating supply,
    /// in wei per whole token — what the next token roughly costs to buy, or
    /// pays out to sell (before the 1.5% fee). Both `buy()` and `sell()` price
    /// off the same `_circulatingSupply()` basis, so there's a single current
    /// price, not separate buy/sell quotes. Informational only — actual
    /// trades always use the exact integral over the traded range via
    /// `_curveCost`, not this spot price.
    function currentPriceWeiPerToken() external view returns (uint256) {
        uint256 s0 = _circulatingSupply();
        return (m * s0) / (TOKEN_UNIT * CURVE_PRICE_SCALE) + b / CURVE_PRICE_SCALE;
    }

    /// @notice Exact wei cost to buy `tokenAmount` token-wei right now (before
    /// any other transaction lands first — not a firm quote). Matches `buy()`
    /// exactly regardless of how much would be filled from vault stock versus
    /// freshly minted — both price identically (see `buy()` NatSpec).
    function previewBuyCost(uint256 tokenAmount) external view returns (uint256) {
        uint256 s0 = _circulatingSupply();
        return _curveCost(s0, s0 + tokenAmount, true);
    }

    /// @notice Exact net/fee wei proceeds for selling `tokenAmount` token-wei
    /// right now (before any other transaction lands first — not a firm quote).
    function previewSellProceeds(uint256 tokenAmount) external view returns (uint256 net, uint256 fee) {
        uint256 s1 = _circulatingSupply();
        if (tokenAmount > s1) revert ExceedsSupply();
        uint256 s0 = s1 - tokenAmount;
        uint256 gross = _curveCost(s0, s1, false);
        fee = (gross * SELL_FEE_BPS) / BPS_DENOMINATOR;
        net = gross - fee;
    }

    /// @notice Token-wei a World ID claim mints — always the flat
    /// CLAIM_AMOUNT_RAW, independent of curve price or supply.
    function previewClaimAmount() external pure returns (uint256) {
        return CLAIM_AMOUNT_RAW;
    }

    /// @notice Token-wei of `account`'s balance still locked under an active
    /// World ID claim lock (0 if none, or if the lock has expired).
    function lockedBalanceOf(address account) external view returns (uint256) {
        return _lockedBalanceOf(account);
    }

    /// @notice Token-wei of `account`'s balance currently sellable back to the
    /// curve (balance minus whatever's still locked, floored at 0 — see the
    /// comment in `sell()` on why this can't simply subtract).
    function sellableBalanceOf(address account) external view returns (uint256) {
        uint256 bal = balanceOf(account);
        uint256 locked = _lockedBalanceOf(account);
        return bal > locked ? bal - locked : 0;
    }

    /// @notice Builder-vesting wei releasable right now.
    function vestingReleasable() external view returns (uint256) {
        return _vestedAmount(block.timestamp) - vestingReleasedWei;
    }

    /// @notice Real-time, on-chain-checkable solvency snapshot — no need to
    /// trust an assumption about fees accumulating; these are the two actual
    /// numbers that matter, compared directly.
    ///
    /// `vaultBalanceWei` is real ETH this contract actually holds right now.
    /// `fullUnwindCostWei` is what it would cost to pay out every token still
    /// actually circulating if it were all sold back in one shot (the same
    /// `_curveCost` integral `sell()` itself uses, from the top of
    /// `_circulatingSupply()` down to 0) — this includes claimed tokens and
    /// any released builder-vesting tokens, since `sell()` doesn't distinguish
    /// their origin either; all circulating supply is fungible once minted.
    /// Tokens the vault itself is already holding (sold back, not burned —
    /// see `sell()`) are deliberately excluded: they're not at risk of being
    /// redeemed a second time, so counting them here would overstate what the
    /// vault actually needs to cover.
    ///
    /// `reserveRatioBps` is `vaultBalanceWei / fullUnwindCostWei` in basis
    /// points. 10_000 (100%) means the vault could pay out a full simultaneous
    /// unwind of every token right now. Below 100% is not evidence of a bug
    /// or dishonesty — claim() mints tokens with zero ETH behind them by
    /// design (see CLAIM_AMOUNT_RAW), so the ratio starts below 100% the
    /// moment any claim happens and only comes back up as real buying adds
    /// ETH to the vault. buy() always raises or holds the ratio, never
    /// worsens it: the ETH a buyer pays is `_curveCost` over the exact same
    /// range that fullUnwindCostWei grows by (both keyed off
    /// `_circulatingSupply()`, see `buy()`), so the two move together
    /// essentially 1:1 regardless of whether the purchase is filled from
    /// vault stock, a fresh mint, or a mix of both.
    ///
    /// sell()'s effect on the ratio is NOT one-directional — this is worth
    /// being precise about rather than assuming fees make it safe by
    /// default. Selling a slice worth `g` (curve cost) pays out `0.985g` from
    /// the vault while removing the full `g` from fullUnwindCostWei. Working
    /// through the algebra: the ratio only rises (or holds) if it was
    /// already >= 98.5% *before* that sell; below that, selling pulls the
    /// ratio DOWN further, not up — because sell() checks only that the
    /// TOTAL vault balance covers the payout, not that the specific slice
    /// being sold was ever funded. Concretely: if claimed (unbacked) tokens
    /// happen to sit at the current top of circulating supply (sell() always
    /// draws from the top), a claim-holder selling can draw down ETH that real buyers
    /// deposited for a *different*, lower slice — the contract cannot tell
    /// the difference, all supply is fungible. So a healthy (near-100%)
    /// ratio tends to stay healthy through ordinary selling, but a already-
    /// underbacked ratio (heavy claiming relative to buying) gets WORSE as
    /// people sell, not better. Sustained recovery requires real net
    /// buy-side demand exceeding sell-side outflow, not fees, and not time
    /// alone.
    function reserveStatus()
        external
        view
        returns (uint256 vaultBalanceWei, uint256 fullUnwindCostWei, uint256 reserveRatioBps)
    {
        vaultBalanceWei = address(this).balance;
        uint256 s1 = _circulatingSupply();
        fullUnwindCostWei = _curveCost(0, s1, false);
        reserveRatioBps = fullUnwindCostWei == 0 ? BPS_DENOMINATOR : (vaultBalanceWei * BPS_DENOMINATOR) / fullUnwindCostWei;
    }
}
