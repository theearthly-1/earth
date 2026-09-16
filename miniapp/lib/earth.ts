import type { IDKitResult, IDKitErrorCodes } from "@worldcoin/idkit-core";
import { createPublicClient, http } from "viem";
import earthAbi from "@/lib/earth_abi.json";

export const EARTH_ABI = earthAbi;

export const EARTH_CONTRACT_ADDRESS = process.env.NEXT_PUBLIC_EARTH_CONTRACT_ADDRESS as `0x${string}`;

export const publicClient = createPublicClient({
  transport: http(process.env.NEXT_PUBLIC_WORLDCHAIN_RPC_URL),
});

/** Checks EARTH.sol's hasClaimed(address) directly — avoids sending a user
 * through the whole verify flow only to hit "nullifier already used" at the
 * very end. */
export async function hasAlreadyClaimed(address: `0x${string}`): Promise<boolean> {
  return publicClient.readContract({
    address: EARTH_CONTRACT_ADDRESS,
    abi: EARTH_ABI,
    functionName: "hasClaimed",
    args: [address],
  }) as Promise<boolean>;
}

/** Rejects if `promise` doesn't settle within `ms` — MiniKit's native bridge
 * calls (walletAuth, sendTransaction) have no built-in timeout, so a
 * suspended/killed WebView can otherwise hang the UI forever. */
export function withTimeout<T>(promise: Promise<T>, ms: number, label: string): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`${label} timed out — try again.`)), ms);
    promise.then(
      (v) => { clearTimeout(timer); resolve(v); },
      (e) => { clearTimeout(timer); reject(e); }
    );
  });
}

const FRIENDLY_ERRORS: Partial<Record<IDKitErrorCodes | string, string>> = {
  nullifier_replayed: "This World ID has already claimed EARTH.",
  duplicate_nonce: "That verification attempt expired — please try again.",
  user_rejected: "Verification was cancelled.",
  verification_rejected: "World ID verification was rejected.",
  credential_unavailable: "Your World ID doesn't have the credential needed to verify.",
  timestamp_too_old: "That verification attempt expired — please try again.",
  rp_signature_expired: "That verification attempt expired — please try again.",
};

export function friendlyIdKitError(code: string): string {
  return FRIENDLY_ERRORS[code] ?? `World ID verification failed (${code}).`;
}

/** MiniKit's sendTransaction only returns a userOpHash, not a final result —
 * poll the actual outcome rather than assuming submission == success. */
export async function pollUserOpStatus(
  userOpHash: string,
  { intervalMs = 2000, timeoutMs = 60000 } = {}
): Promise<"success" | "failed"> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const res = await fetch(`https://developer.world.org/api/v2/minikit/userop/${userOpHash}`);
    if (res.ok) {
      const body = await res.json();
      if (body.status === "success" || body.status === "failed") return body.status;
    }
    await new Promise((r) => setTimeout(r, intervalMs));
  }
  throw new Error("Timed out confirming your claim — check your wallet activity to see if it went through.");
}

// World Chain's own chain ID (MiniKit's sendTransaction command is scoped to
// this — it will reject any other chain ID outright). Sepolia testnet is a
// SEPARATE chain ID (4801); confirm with MiniKit/World App docs whether
// sendTransaction supports it before relying on this for testnet-in-app
// testing — the reference example we found only documents mainnet's 480.
export const WORLD_CHAIN_ID = 480;

/**
 * Converts a World ID `rp_id` string (e.g. "rp_b8cbc8572950bbd4") into the
 * uint64 the contract's claim() expects. The 16 hex characters after the
 * "rp_" prefix are exactly 8 bytes — the right width for a uint64 — so this
 * treats them as that value directly. This inference isn't from official
 * docs (the research pass didn't find a documented conversion); confirm
 * against a real Developer Portal example before a real deploy.
 */
export function rpIdToUint64(rpId: string): bigint {
  const hex = rpId.replace(/^rp_/, "");
  if (!/^[0-9a-fA-F]{16}$/.test(hex)) {
    throw new Error(`Unexpected rp_id format: "${rpId}" — expected "rp_" + 16 hex chars.`);
  }
  return BigInt(`0x${hex}`);
}

/** UUID (32 hex chars once dashes are stripped, i.e. 128 bits) as a uint256. */
export function nonceUuidToUint256(nonceUuid: string): bigint {
  const hex = nonceUuid.replace(/-/g, "");
  if (!/^[0-9a-fA-F]{32}$/.test(hex)) {
    throw new Error(`Unexpected nonce format: "${nonceUuid}" — expected a UUID.`);
  }
  return BigInt(`0x${hex}`);
}

function hexToUint256(hex: string): bigint {
  return BigInt(hex.startsWith("0x") ? hex : `0x${hex}`);
}

/** Maps IDKit's World ID 4.0 response into claim()'s 7 on-chain arguments. */
export function toClaimArgs(idkitResult: IDKitResult, rpId: string) {
  if (idkitResult.protocol_version !== "4.0" || "session_id" in idkitResult) {
    throw new Error(
      `Expected a World ID 4.0 uniqueness proof, got protocol_version=${idkitResult.protocol_version}` +
        ("session_id" in idkitResult ? " (session proof)" : " (legacy v3 proof)")
    );
  }

  const r = idkitResult.responses[0];
  if (!r) throw new Error("World ID response had no proof entries.");
  if (r.proof.length !== 5) throw new Error(`Expected a 5-element proof, got ${r.proof.length}.`);

  return [
    hexToUint256(r.nullifier), // nullifier
    rpIdToUint64(rpId), // rpId
    nonceUuidToUint256(idkitResult.nonce), // nonce
    BigInt(r.expires_at_min), // expiresAtMin
    BigInt(r.issuer_schema_id), // issuerSchemaId
    0n, // credentialGenesisIssuedAtMin — unconstrained
    r.proof.map(hexToUint256) as [bigint, bigint, bigint, bigint, bigint], // zeroKnowledgeProof
  ] as const;
}
