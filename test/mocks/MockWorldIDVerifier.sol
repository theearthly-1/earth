// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {IWorldIDVerifier} from "../../contracts/EARTH.sol";

contract MockWorldIDVerifier is IWorldIDVerifier {
    bool public shouldRevert;

    function setShouldRevert(bool value) external {
        shouldRevert = value;
    }

    function verify(
        uint256,
        uint256,
        uint64,
        uint256,
        uint256,
        uint64,
        uint64,
        uint256,
        uint256[5] calldata
    ) external view override {
        if (shouldRevert) revert("MockWorldIDVerifier: invalid proof");
    }
}
