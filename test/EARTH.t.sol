// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Test} from "forge-std/Test.sol";
import {EARTH} from "../contracts/EARTH.sol";
import {MockWorldIDVerifier} from "./mocks/MockWorldIDVerifier.sol";

contract EARTHTest is Test {
    EARTH earth;
    MockWorldIDVerifier verifier;

    address admin = makeAddr("admin");
    address multisig = makeAddr("multisig");
    address alice = makeAddr("alice");
    address bob = makeAddr("bob");

    uint256 constant CURVE_M = 1_000_000; // scaled by CURVE_PRICE_SCALE (1e6) -> real m = 1
    uint256 constant CURVE_B = 1_000_000_000; // scaled -> real b = 1000 wei/token at zero supply
    uint256 constant CLAIM_ACTION_ID = 12345;

    function setUp() public {
        verifier = new MockWorldIDVerifier();
        vm.prank(admin);
        earth = new EARTH(CURVE_M, CURVE_B, address(verifier), CLAIM_ACTION_ID, multisig, admin);
        vm.deal(alice, 1000 ether);
        vm.deal(bob, 1000 ether);
    }

    function _validProof() internal pure returns (uint256[5] memory proof) {}

    function _claim(address who, uint256 nullifier) internal {
        vm.prank(who);
        earth.claim(nullifier, 0, 0, 0, 0, 0, _validProof());
    }

    function _buy(address who, uint256 amount) internal returns (uint256 cost) {
        cost = earth.previewBuyCost(amount);
        vm.prank(who);
        earth.buy{value: cost}(amount, cost);
    }

    // ==================== constructor ====================

    function test_constructor_setsImmutables() public view {
        assertEq(earth.m(), CURVE_M);
        assertEq(earth.b(), CURVE_B);
        assertEq(earth.CLAIM_ACTION_ID(), CLAIM_ACTION_ID);
        assertEq(earth.builderVestingMultisig(), multisig);
        assertEq(earth.owner(), admin);
        assertEq(earth.totalSupply(), 0);
    }

    function test_constructor_revertsOnZeroCurveM() public {
        vm.expectRevert(EARTH.InvalidCurveParams.selector);
        new EARTH(0, CURVE_B, address(verifier), CLAIM_ACTION_ID, multisig, admin);
    }

    function test_constructor_revertsOnZeroCurveB() public {
        vm.expectRevert(EARTH.InvalidCurveParams.selector);
        new EARTH(CURVE_M, 0, address(verifier), CLAIM_ACTION_ID, multisig, admin);
    }

    function test_constructor_revertsOnCurveMTooLarge() public {
        uint256 tooLarge = earth.MAX_CURVE_M() + 1;
        vm.expectRevert(EARTH.InvalidCurveParams.selector);
        new EARTH(tooLarge, CURVE_B, address(verifier), CLAIM_ACTION_ID, multisig, admin);
    }

    function test_constructor_revertsOnZeroVerifier() public {
        vm.expectRevert(EARTH.ZeroAddress.selector);
        new EARTH(CURVE_M, CURVE_B, address(0), CLAIM_ACTION_ID, multisig, admin);
    }

    function test_constructor_revertsOnZeroMultisig() public {
        vm.expectRevert(EARTH.ZeroAddress.selector);
        new EARTH(CURVE_M, CURVE_B, address(verifier), CLAIM_ACTION_ID, address(0), admin);
    }

    // ==================== buy ====================

    function test_buy_mintsExactAmount() public {
        uint256 amount = 1000e18;
        uint256 cost = _buy(alice, amount);
        assertEq(earth.balanceOf(alice), amount);
        assertEq(earth.totalSupply(), amount);
        assertGt(cost, 0);
    }

    function test_buy_refundsExcessPayment() public {
        uint256 amount = 1000e18;
        uint256 cost = earth.previewBuyCost(amount);
        uint256 balBefore = alice.balance;
        vm.prank(alice);
        earth.buy{value: cost + 5 ether}(amount, cost + 5 ether);
        assertEq(alice.balance, balBefore - cost);
    }

    function test_buy_revertsOnSlippage() public {
        uint256 amount = 1000e18;
        uint256 cost = earth.previewBuyCost(amount);
        vm.prank(alice);
        vm.expectRevert(abi.encodeWithSelector(EARTH.SlippageExceeded.selector, cost, cost - 1));
        earth.buy{value: cost}(amount, cost - 1);
    }

    function test_buy_revertsOnInsufficientPayment() public {
        uint256 amount = 1000e18;
        uint256 cost = earth.previewBuyCost(amount);
        vm.prank(alice);
        vm.expectRevert(abi.encodeWithSelector(EARTH.InsufficientPayment.selector, cost - 1, cost));
        earth.buy{value: cost - 1}(amount, cost);
    }

    function test_buy_revertsOnZeroAmount() public {
        vm.prank(alice);
        vm.expectRevert(EARTH.ZeroAmount.selector);
        earth.buy{value: 1 ether}(0, 1 ether);
    }

    function test_buy_priceIncreasesAlongCurve() public {
        uint256 amount = 10_000e18;
        uint256 firstCost = earth.previewBuyCost(amount);
        _buy(alice, amount);
        uint256 secondCost = earth.previewBuyCost(amount);
        assertGt(secondCost, firstCost);
    }

    function test_buy_revertsWhenExceedingCapReservingVestingRoom() public {
        uint256 fullSupply = earth.MAX_SUPPLY_WEI();
        uint256 cost = earth.previewBuyCost(fullSupply);
        vm.deal(alice, cost);
        vm.prank(alice);
        vm.expectRevert(EARTH.ExceedsCap.selector);
        earth.buy{value: cost}(fullSupply, cost);
    }

    function test_buy_revertsWhenPricingRangeWouldExceedMaxSupply() public {
        // Any nonzero existing supply plus a full-MAX_SUPPLY_WEI request pushes
        // the *pricing* range (s0 + tokenAmount) over MAX_SUPPLY_WEI, even
        // though such a huge buy would already revert via ExceedsCap for an
        // unrelated reason — this test isolates the new, separate safety
        // check added specifically because buy() can now be partly filled
        // from vault stock rather than always minting the full amount.
        _claim(alice, 1); // totalSupply() = 1000e18, > 0
        uint256 fullSupply = earth.MAX_SUPPLY_WEI();
        vm.deal(bob, 1); // doesn't matter, reverts before payment is checked
        vm.prank(bob);
        vm.expectRevert(EARTH.ExceedsSupply.selector);
        earth.buy{value: 1}(fullSupply, type(uint256).max);
    }

    function testFuzz_previewBuyCost_matchesActualCost(uint96 rawAmount) public {
        uint256 amount = bound(uint256(rawAmount), 1, 100_000e18);
        uint256 cost = earth.previewBuyCost(amount);
        vm.deal(alice, cost);
        vm.prank(alice);
        earth.buy{value: cost}(amount, cost);
        assertEq(earth.balanceOf(alice), amount);
    }

    function test_buy_recyclesVaultStockBeforeMinting() public {
        _buy(alice, 1000e18);
        vm.prank(alice);
        earth.sell(1000e18, 0); // vault now holds 1000e18 EARTH, never burned
        assertEq(earth.balanceOf(address(earth)), 1000e18);
        uint256 supplyBeforeBobsBuy = earth.totalSupply();

        _buy(bob, 500e18); // fully covered by vault stock, well under it

        assertEq(earth.balanceOf(bob), 500e18);
        assertEq(earth.balanceOf(address(earth)), 500e18); // vault stock drawn down
        assertEq(earth.totalSupply(), supplyBeforeBobsBuy); // nothing new minted
    }

    function test_buy_mintsOnlyThePortionExceedingVaultStock() public {
        _buy(alice, 1000e18);
        vm.prank(alice);
        earth.sell(300e18, 0); // vault stock = 300e18
        uint256 supplyBefore = earth.totalSupply();

        _buy(bob, 1000e18); // 300e18 from stock, 700e18 freshly minted

        assertEq(earth.balanceOf(bob), 1000e18);
        assertEq(earth.balanceOf(address(earth)), 0); // stock fully drawn down
        assertEq(earth.totalSupply(), supplyBefore + 700e18); // only the excess minted
    }

    function test_buy_priceMovesNaturallyWithBuysAndSells() public {
        // The price is NOT a one-way ratchet — it should behave exactly like
        // it always did: rise when people buy, fall back when they sell.
        // What's different from before is only that nothing gets destroyed;
        // the price dynamics themselves are unchanged.
        uint256 costAtZeroSupply = earth.previewBuyCost(1000e18);

        _buy(alice, 100_000e18); // pushes the curve up
        uint256 costWhilePumped = earth.previewBuyCost(1000e18);
        assertGt(costWhilePumped, costAtZeroSupply);

        vm.prank(alice);
        earth.sell(100_000e18, 0); // sells it ALL back — nothing burned, held as vault stock
        assertEq(earth.balanceOf(alice), 0);
        assertEq(earth.totalSupply(), 100_000e18); // totalSupply never dropped — nothing destroyed

        uint256 costAfterFullSellback = earth.previewBuyCost(1000e18);
        assertApproxEqRel(costAfterFullSellback, costAtZeroSupply, 0.01e18); // price naturally returned to normal
    }

    function test_reserveStatus_cushionGrowsFromFeesAcrossManyTrades() public {
        // The thing that's actually supposed to trend upward over time isn't
        // the price — it's the vault's real backing, because every sell
        // keeps 1.5% behind. Buy and sell the same amount back and forth
        // repeatedly and confirm the vault balance strictly grows each round,
        // even though the price itself just oscillates around the same level.
        uint256 amount = 1000e18;
        uint256 vaultBalanceBefore;
        uint256 vaultBalanceAfter;

        for (uint256 i = 0; i < 5; i++) {
            (vaultBalanceBefore, , ) = earth.reserveStatus();
            _buy(alice, amount);
            vm.prank(alice);
            earth.sell(amount, 0);
            (vaultBalanceAfter, , ) = earth.reserveStatus();
            assertGt(vaultBalanceAfter, vaultBalanceBefore); // cushion grew this round
        }
    }

    // ==================== sell ====================

    function test_sell_transfersToVaultAndPaysOutNetOfFee() public {
        uint256 amount = 1000e18;
        _buy(alice, amount);
        uint256 supplyBefore = earth.totalSupply();
        (uint256 net, uint256 fee) = earth.previewSellProceeds(amount);
        uint256 balBefore = alice.balance;
        vm.prank(alice);
        earth.sell(amount, net);
        assertEq(earth.balanceOf(alice), 0);
        assertEq(alice.balance, balBefore + net);
        assertGt(fee, 0);
        // The actual point of tonight's change: nothing is burned.
        assertEq(earth.totalSupply(), supplyBefore); // unchanged, not reduced
        assertEq(earth.balanceOf(address(earth)), amount); // sold tokens now sit in the vault
    }

    function test_sell_revertsOnInsufficientBalance() public {
        vm.prank(alice);
        vm.expectRevert(abi.encodeWithSelector(EARTH.InsufficientBalance.selector, 0, 1e18));
        earth.sell(1e18, 0);
    }

    function test_sell_pricesCorrectlyAcrossMultipleSequentialSells() public {
        // Guards against the exact bug this whole redesign had to avoid: if
        // sell() priced off raw totalSupply() (which no longer drops), every
        // sequential seller would get quoted the same top-of-curve price
        // regardless of order. Pricing off _circulatingSupply() instead means
        // each successive sell must price a lower band than the last.
        _buy(alice, 2000e18);

        vm.prank(alice);
        uint256 balBefore1 = alice.balance;
        earth.sell(1000e18, 0); // sells the top band [1000e18, 2000e18]
        uint256 proceeds1 = alice.balance - balBefore1;

        uint256 balBefore2 = alice.balance;
        vm.prank(alice);
        earth.sell(1000e18, 0); // sells the bottom band [0, 1000e18] — must be cheaper
        uint256 proceeds2 = alice.balance - balBefore2;

        assertGt(proceeds1, proceeds2);
    }

    function test_buySell_roundTripLosesToFee() public {
        uint256 amount = 1000e18;
        uint256 cost = _buy(alice, amount);
        (uint256 net,) = earth.previewSellProceeds(amount);
        vm.prank(alice);
        earth.sell(amount, net);
        assertLt(net, cost);
    }

    // ==================== claim ====================

    function test_claim_mintsFlatAmount() public {
        _claim(alice, 1);
        assertEq(earth.balanceOf(alice), earth.CLAIM_AMOUNT_RAW());
        assertEq(earth.CLAIM_AMOUNT_RAW(), 1000e18);
        assertEq(earth.previewClaimAmount(), 1000e18);
    }

    function test_claim_isIndependentOfCurvePriceAndSupply() public {
        _buy(alice, 500_000e18); // move the curve a lot first
        _claim(bob, 1);
        assertEq(earth.balanceOf(bob), 1000e18);
    }

    function test_claim_locksTokensFor30Days() public {
        _buy(bob, 1_000_000e18); // fund the vault so the later sell has reserves to pay out
        _claim(alice, 1);

        vm.prank(alice);
        vm.expectRevert(abi.encodeWithSelector(EARTH.TokensLocked.selector, 0, 1000e18));
        earth.sell(1000e18, 0);

        vm.warp(block.timestamp + 30 days);
        vm.prank(alice);
        earth.sell(1000e18, 0);
        assertEq(earth.balanceOf(alice), 0);
    }

    function test_claim_revertsOnReusedNullifier() public {
        _claim(alice, 1);
        vm.prank(bob);
        vm.expectRevert(EARTH.AlreadyClaimed.selector);
        earth.claim(1, 0, 0, 0, 0, 0, _validProof());
    }

    function test_claim_revertsOnSecondClaimBySameAddress() public {
        _claim(alice, 1);
        vm.prank(alice);
        vm.expectRevert(EARTH.AlreadyClaimed.selector);
        earth.claim(2, 0, 0, 0, 0, 0, _validProof());
    }

    function test_claim_revertsOnInvalidProof() public {
        verifier.setShouldRevert(true);
        vm.prank(alice);
        vm.expectRevert("MockWorldIDVerifier: invalid proof");
        earth.claim(1, 0, 0, 0, 0, 0, _validProof());
    }

    function test_claim_purchasedTokensRemainSellableAlongsideLockedClaim() public {
        _buy(bob, 1_000_000e18); // fund the vault so alice's sell below has reserves to pay out
        _buy(alice, 5000e18);
        _claim(alice, 1);
        assertEq(earth.sellableBalanceOf(alice), 5000e18);
        vm.prank(alice);
        earth.sell(5000e18, 0);
        assertEq(earth.balanceOf(alice), 1000e18); // locked claim tokens remain
    }

    // ==================== admin ====================

    function test_setWorldIdVerifier_onlyOwner() public {
        address newVerifier = makeAddr("newVerifier");
        vm.prank(alice);
        vm.expectRevert(abi.encodeWithSignature("OwnableUnauthorizedAccount(address)", alice));
        earth.setWorldIdVerifier(newVerifier);

        vm.prank(admin);
        earth.setWorldIdVerifier(newVerifier);
        assertEq(address(earth.worldIdVerifier()), newVerifier);
    }

    function test_renounceAdmin_disablesAdminForever() public {
        vm.prank(admin);
        earth.renounceAdmin();
        assertEq(earth.owner(), address(0));

        vm.prank(admin);
        vm.expectRevert(abi.encodeWithSignature("OwnableUnauthorizedAccount(address)", admin));
        earth.setWorldIdVerifier(makeAddr("x"));
    }

    // ==================== builder vesting ====================

    function test_releaseVesting_revertsBeforeCliff() public {
        vm.prank(multisig);
        vm.expectRevert(EARTH.NothingToRelease.selector);
        earth.releaseVesting();
    }

    function test_releaseVesting_revertsForNonMultisig() public {
        vm.warp(block.timestamp + 181 days);
        vm.prank(alice);
        vm.expectRevert(EARTH.NotVestingRecipient.selector);
        earth.releaseVesting();
    }

    function test_releaseVesting_linearAfterCliff() public {
        vm.warp(block.timestamp + 180 days + 300 days); // halfway through the 600-day vest
        vm.prank(multisig);
        earth.releaseVesting();
        uint256 fullAllocation = earth.BUILDER_VESTING_ALLOCATION_WEI();
        assertApproxEqRel(earth.balanceOf(multisig), fullAllocation / 2, 0.01e18);
    }

    function test_releaseVesting_fullyVestedAfterSchedule() public {
        vm.warp(block.timestamp + 180 days + 600 days + 1);
        vm.prank(multisig);
        earth.releaseVesting();
        assertEq(earth.balanceOf(multisig), earth.BUILDER_VESTING_ALLOCATION_WEI());

        vm.prank(multisig);
        vm.expectRevert(EARTH.NothingToRelease.selector);
        earth.releaseVesting();
    }

    // ==================== reserveStatus ====================

    function test_reserveStatus_zeroSupplyIsFullyBacked() public view {
        (uint256 vault, uint256 unwind, uint256 ratio) = earth.reserveStatus();
        assertEq(vault, 0);
        assertEq(unwind, 0);
        assertEq(ratio, 10_000);
    }

    function test_reserveStatus_claimAloneIsZeroBacked() public {
        _claim(alice, 1);
        (uint256 vault, uint256 unwind, uint256 ratio) = earth.reserveStatus();
        assertEq(vault, 0);
        assertGt(unwind, 0);
        assertEq(ratio, 0);
    }

    function test_reserveStatus_buyAloneIsFullyBacked() public {
        _buy(alice, 1000e18);
        (uint256 vault, uint256 unwind, uint256 ratio) = earth.reserveStatus();
        assertEq(vault, unwind);
        assertEq(ratio, 10_000);
    }

    function test_reserveStatus_excludesVaultHeldStockFromUnwindCost() public {
        _buy(alice, 1000e18);
        vm.prank(alice);
        earth.sell(1000e18, 0); // vault now holds all 1000e18 back as stock; circulating supply is 0

        (uint256 vault, uint256 unwind, uint256 ratio) = earth.reserveStatus();
        assertEq(earth.totalSupply(), 1000e18); // still exists, just not burned
        assertEq(unwind, 0); // nothing actually circulating needs to be paid out
        assertGt(vault, 0); // the 1.5% fee retained is still real ETH sitting there
        assertEq(ratio, 10_000); // vacuously "fully backed" — nothing is owed
    }

    // ==================== spot price views ====================

    function test_currentPrice_dropsAfterSellback() public {
        // Confirms there is one shared price for buying and selling, and it
        // moves naturally — a full sellback brings it back down, it does not
        // stay pinned at the peak.
        uint256 priceAtZero = earth.currentPriceWeiPerToken();
        _buy(alice, 100_000e18);
        uint256 priceAfterBuy = earth.currentPriceWeiPerToken();
        assertGt(priceAfterBuy, priceAtZero);

        vm.prank(alice);
        earth.sell(100_000e18, 0);
        uint256 priceAfterSellback = earth.currentPriceWeiPerToken();
        assertEq(priceAfterSellback, priceAtZero); // exact match: circulating supply is back to 0
    }
}
