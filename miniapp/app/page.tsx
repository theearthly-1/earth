"use client";

import { useCallback, useEffect, useState } from "react";
import { MiniKit } from "@worldcoin/minikit-js";
import { IDKit, proofOfHuman, deviceLegacy } from "@worldcoin/idkit-core";
import { encodeFunctionData } from "viem";
import {
  EARTH_ABI,
  EARTH_CONTRACT_ADDRESS,
  WORLD_CHAIN_ID,
  toClaimArgs,
  hasAlreadyClaimed,
  withTimeout,
  friendlyIdKitError,
  pollUserOpStatus,
} from "@/lib/earth";

type Status = "idle" | "verifying" | "claiming" | "confirming" | "done" | "already-claimed" | "error";

const WORLD_ACTION_ID = process.env.NEXT_PUBLIC_WORLD_ACTION_ID!;
const WORLD_RP_ID = process.env.NEXT_PUBLIC_WORLD_RP_ID!;
const STAGING = process.env.NEXT_PUBLIC_WORLD_ID_ENVIRONMENT === "staging";

export default function ClaimPage() {
  const [miniKitReady, setMiniKitReady] = useState(false);
  const [wallet, setWallet] = useState<string | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [status, setStatus] = useState<Status>("idle");
  const [message, setMessage] = useState("");

  useEffect(() => {
    setMiniKitReady(MiniKit.isInstalled());
  }, []);

  const handleConnect = useCallback(async () => {
    if (connecting) return; // guards double-tap: MiniKit's event bridge only tracks one in-flight listener per event type
    setConnecting(true);
    try {
      // Anti-replay nonce for the SIWE message walletAuth signs. Normally
      // server-issued so a backend can reject reused nonces, but this app
      // has no backend at all by design (that's the whole point of on-chain
      // World ID verification) — walletAuth here is only used to learn which
      // address to bind the World ID proof's signal to, not for a server
      // session, so a client-generated nonce is fine for that narrower job.
      const nonce = crypto.randomUUID().replace(/-/g, "");
      const result = await withTimeout(
        MiniKit.walletAuth({ nonce, statement: "Connect your wallet to claim EARTH." }),
        30000,
        "Wallet connection"
      );
      const address = result.data.address as `0x${string}`;
      setWallet(address);
      setStatus("idle");
      setMessage("");

      const already = await hasAlreadyClaimed(address).catch(() => false);
      if (already) {
        setStatus("already-claimed");
        setMessage("This wallet has already claimed its EARTH.");
      }
    } catch (err) {
      setStatus("error");
      setMessage(err instanceof Error ? err.message : "Failed to connect wallet.");
    } finally {
      setConnecting(false);
    }
  }, [connecting]);

  const handleClaim = useCallback(async () => {
    if (!wallet) {
      setMessage("Connect your wallet first.");
      return;
    }

    try {
      // 1. Prove unique humanity, bound to this wallet address (as the
      //    signal) so the proof can't be replayed against a different one.
      //    The contract independently recomputes this same signal hash from
      //    msg.sender on-chain — it must match exactly, or verification
      //    fails closed.
      setStatus("verifying");
      setMessage("Waiting for World ID verification...");

      const rpContext = await fetch("/api/rp-signature", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ action: WORLD_ACTION_ID }),
      }).then((r) => r.json());
      if (rpContext.error) throw new Error(rpContext.error);

      const request = await IDKit.request({
        app_id: process.env.NEXT_PUBLIC_WORLD_APP_ID as `app_${string}`,
        action: WORLD_ACTION_ID,
        rp_context: rpContext,
        // deviceLegacy() only ever returns v3 proofs, so it MUST be paired
        // with allow_legacy_proofs: true or the request is self-contradictory
        // and can never complete. proofOfHuman() is real v4 sybil resistance
        // and doesn't need the legacy fallback.
        allow_legacy_proofs: STAGING,
      }).preset(
        // proofOfHuman requires a real Orb-verified credential, which test
        // accounts don't have. deviceLegacy only needs basic device
        // registration — lowest bar that still exercises the full pipeline.
        // This MUST be proofOfHuman before real users claim — device-level
        // verification alone isn't real sybil resistance.
        STAGING ? deviceLegacy({ signal: wallet }) : proofOfHuman({ signal: wallet })
      );

      const completion = await request.pollUntilCompletion({ timeout: 180000 });
      if (!completion.success) {
        throw new Error(friendlyIdKitError(completion.error));
      }

      // 2. Submit the claim transaction directly on-chain — MiniKit sends it
      //    from whatever wallet is active in this World App session. That's
      //    normally the same one connected above, but if it isn't (account
      //    switch mid-flow), the signal the proof was bound to won't match
      //    msg.sender and the contract rejects it — check proactively so the
      //    failure is diagnosable instead of a raw revert.
      setStatus("claiming");
      setMessage("Submitting your claim on-chain...");

      const args = toClaimArgs(completion.result, WORLD_RP_ID);
      const data = encodeFunctionData({
        abi: EARTH_ABI,
        functionName: "claim",
        args,
      });

      const send = await withTimeout(
        MiniKit.sendTransaction({
          chainId: WORLD_CHAIN_ID,
          transactions: [{ to: EARTH_CONTRACT_ADDRESS, data }],
        }),
        60000,
        "Transaction submission"
      );

      if (send.data.from && send.data.from.toLowerCase() !== wallet.toLowerCase()) {
        throw new Error(
          "Your active wallet changed since you connected — reconnect and try again."
        );
      }

      // sendTransaction only confirms the operation was SUBMITTED, not that
      // it succeeded on-chain — poll the real outcome before calling it done.
      setStatus("confirming");
      setMessage("Confirming your claim on-chain...");
      const outcome = await pollUserOpStatus(send.data.userOpHash);
      if (outcome === "failed") {
        throw new Error("Your claim transaction failed on-chain — no tokens were minted.");
      }

      setStatus("done");
      setMessage("Claimed! 1,000 EARTH is on its way to your wallet.");
    } catch (err) {
      setStatus("error");
      setMessage(err instanceof Error ? err.message : "Something went wrong.");
    }
  }, [wallet]);

  const busy = status === "verifying" || status === "claiming" || status === "confirming";
  const claimDisabled = busy || !wallet || status === "done" || status === "already-claimed";

  if (!miniKitReady) {
    return (
      <main style={{ maxWidth: 420, margin: "80px auto", padding: 24, textAlign: "center" }}>
        <h1>Claim your EARTH</h1>
        <p style={{ color: "#b45309" }}>Open this page inside World App to claim.</p>
      </main>
    );
  }

  return (
    <main style={{ maxWidth: 420, margin: "80px auto", padding: 24, textAlign: "center" }}>
      <h1>Claim your EARTH</h1>
      <p>Verify you&apos;re a unique human, then claim your 1,000 EARTH.</p>

      {wallet ? (
        <p>
          Connected: {wallet.slice(0, 6)}...{wallet.slice(-4)}
        </p>
      ) : (
        <button onClick={handleConnect} disabled={connecting} style={{ padding: "10px 20px" }}>
          {connecting ? "Connecting..." : "Connect Wallet"}
        </button>
      )}

      <button
        onClick={handleClaim}
        disabled={claimDisabled}
        style={{ marginTop: 16, padding: "12px 24px", fontSize: 16 }}
      >
        {busy
          ? status === "confirming"
            ? "Confirming..."
            : "Working..."
          : status === "done"
            ? "Claimed"
            : status === "already-claimed"
              ? "Already claimed"
              : "Verify & Claim EARTH"}
      </button>

      {message && <p style={{ marginTop: 16 }}>{message}</p>}
    </main>
  );
}
