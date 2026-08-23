# Credential sync is direct, end-to-end encrypted, and never copies OAuth sessions

Quota Widget transfers credentials between a user's own devices with no
first-party server in the path. What moves is a [[credential bundle]]: each
account's entry and, for pasted-key providers, its API key. OAuth and cookie
accounts move as their entries alone and sign in again on the target, so a
rotating session is never copied. There are two transports for the one bundle: a
live [[device pairing]] — over the local network for any device pair, or a code
scanned from a desktop to a phone — and an encrypted [[credential export]] file
for backup and offline provisioning. The job is one-shot provisioning and
disaster recovery, not a standing link that keeps devices in step.

## Considered options

A **first-party sync server** was rejected: it would make us the custodian of
every user's API keys and the single blast radius for their loss, exactly the
posture `docs/research-privy-native-auth.md` declined to take on for auth. The
export file and LAN pairing keep the bytes on devices the user controls.

**Copying OAuth refresh tokens** was rejected. Modern providers rotate them on
every refresh and treat a replayed old token as theft, revoking the whole
session family (Privy's are explicitly rotated and non-shareable; the Windows
backend already splits these oversized JWTs across credentials). Two live
devices sharing one token race to a forced logout. Re-authenticating on the
target — Claude PKCE, Codex device-flow, Hermes cookie, all present on both
platforms — sidesteps the race and costs one sign-in per OAuth account.

**Trusting the LAN** for pairing was rejected: a peer on a shared or café network
could impersonate the other device and silently receive the bundle. Pairing is
instead authenticated by a short code run through a PAKE (SPAKE2 / CPace), so a
network attacker gets a single online guess before the exchange burns.

**Continuous multi-master sync** was rejected in favour of directional,
on-demand transfer: merging secrets invites a bad merge that swaps a good key
for a stale one, and the actual need is provisioning and backup.

## Consequences

The per-platform secret stores (Windows Credential Manager, Android Keystore,
owner-only file on Linux) are unchanged; sync is a transfer layer over them, not
a change to at-rest storage. A transferred OAuth account lands in
[[provider onboarding]] awaiting its first sign-in, not as a
[[pending sign-in]]. The export is protected by an Argon2id-derived key under
XChaCha20-Poly1305 with a versioned header; losing the passphrase makes it
unrecoverable, which is the price of holding no key ourselves. The QR direct
path is bounded by QR capacity and falls back to LAN pairing or the export file
for large fleets. `CONTEXT.md`'s [[shared configuration]] keeps its meaning —
aligned across platforms, entered independently — and ADR-0007's "no cross-device
syncing" now has one bounded exception: the credential bundle, moved once, on
request.
