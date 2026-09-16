import { NextResponse } from "next/server";
import { signRequest } from "@worldcoin/idkit-server";

// Narrow, self-contained: signs the rp_context IDKit.request() requires to
// accept a proof request from this app. This does NOT attest humanness or
// register anything — that's still the ZK proof plus the on-chain nullifier
// check in EARTH.sol's claim(). This key only proves "this request really
// came from this app," a pure-JS signature with no network calls.
//
// Only ever signs our own registered action — not the caller-supplied one.
// Without this check, anyone could POST an arbitrary action string and get
// it validly RP-signed, turning this into an unrestricted signing oracle.
const EXPECTED_ACTION = process.env.NEXT_PUBLIC_WORLD_ACTION_ID;

export async function POST(req: Request) {
  let action: unknown;
  try {
    ({ action } = await req.json());
  } catch {
    return NextResponse.json({ error: "Invalid request body" }, { status: 400 });
  }

  if (!action || typeof action !== "string") {
    return NextResponse.json({ error: "Missing action" }, { status: 400 });
  }
  if (action !== EXPECTED_ACTION) {
    return NextResponse.json({ error: "Unsupported action" }, { status: 403 });
  }

  const signingKeyHex = process.env.RP_SIGNING_KEY;
  if (!signingKeyHex) {
    // Deliberately vague — the specific env var name is an internal detail.
    return NextResponse.json({ error: "Server misconfigured" }, { status: 500 });
  }

  try {
    const { sig, nonce, createdAt, expiresAt } = signRequest({ signingKeyHex, action, ttl: 300 });
    return NextResponse.json({
      rp_id: process.env.NEXT_PUBLIC_WORLD_RP_ID,
      nonce,
      created_at: createdAt,
      expires_at: expiresAt,
      signature: sig,
    });
  } catch {
    return NextResponse.json({ error: "Failed to sign request" }, { status: 500 });
  }
}
