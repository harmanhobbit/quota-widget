# Kimi Code OAuth and usage research

Researched 2026-08-19 against Moonshot AI's public, MIT-licensed
[Kimi Code repository](https://github.com/MoonshotAI/kimi-code), pinned to
[`fa9865f2ee653133295992489554bb2db05a9db5`](https://github.com/MoonshotAI/kimi-code/tree/fa9865f2ee653133295992489554bb2db05a9db5).
This records observed client behaviour, not a promise that Moonshot treats the
endpoints as a public third-party integration contract. Validate it with a
real Kimi Code account before shipping.

## Endpoint and credential summary

| Purpose | Request |
| --- | --- |
| Device authorization | `POST https://auth.kimi.com/api/oauth/device_authorization`, form: `client_id=17e5f671-d194-4dfb-9706-5516cb48c098` |
| Device-token poll | `POST https://auth.kimi.com/api/oauth/token`, form: `client_id`, `device_code`, `grant_type=urn:ietf:params:oauth:grant-type:device_code` |
| Refresh | `POST https://auth.kimi.com/api/oauth/token`, form: `client_id`, `refresh_token`, `grant_type=refresh_token` |
| Kimi Code quota | `GET https://api.kimi.com/coding/v1/usages`, headers: `Authorization: Bearer <access token>`, `Accept: application/json` |

The OAuth host and client ID are hard-coded in the official client (with
environment-only host overrides). The device-authorization request passes
only `client_id`; there is **no `scope` request parameter** in this client.
The returned `scope`, if present, is persisted.

Sources: [flow configuration](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/packages/oauth/src/constants.ts#L3-L21),
[device request](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/packages/oauth/src/oauth.ts#L119-L158),
[poll](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/packages/oauth/src/oauth.ts#L168-L211), and
[refresh](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/packages/oauth/src/oauth.ts#L226-L290).

## Device flow and request headers

All OAuth requests are URL-form encoded with `Content-Type:
application/x-www-form-urlencoded` and `Accept: application/json`. They have a
30-second HTTP timeout. The device response must contain `user_code`,
`device_code`, and `verification_uri_complete`; it may also contain
`verification_uri`, `expires_in`, and a polling `interval` (defaulted to five
seconds by the client).

The CLI supplies these identity headers to the OAuth requests:

- `User-Agent: kimi-code-cli/<version>`
- `X-Msh-Platform: kimi_code_cli`
- `X-Msh-Version: <version>`
- `X-Msh-Device-Name: <hostname>`
- `X-Msh-Device-Model: <OS/version/architecture>`
- `X-Msh-Os-Version: <OS release>`
- `X-Msh-Device-Id: <stable UUID>`

The UUID is created once at the client's home directory as `device_id`.
Kimi Code's source explicitly says each host must declare its own platform;
quota-widget should not claim to be `kimi_code_cli`. Whether Moonshot accepts a
different, truthful client identity needs a real-account validation.

Sources: [form encoding and timeout](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/packages/oauth/src/oauth.ts#L56-L98),
[header fields](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/packages/oauth/src/types.ts#L47-L61),
[identity construction and device ID](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/packages/oauth/src/identity.ts#L42-L105),
[CLI identity](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/apps/kimi-code/src/cli/version.ts#L50-L63), and
[OAuth wiring](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/packages/oauth/src/toolkit.ts#L405-L432).

## Tokens, persistence, and refresh

The token response requires non-empty `access_token`, `refresh_token`, and
positive `expires_in`. It also accepts optional `scope` and `token_type`
(`Bearer` fallback). `expires_at` is calculated as current Unix seconds plus
`expires_in`.

The official CLI stores the following JSON fields in
`~/.kimi-code/credentials/kimi-code.json` (or another provider-name file):
`access_token`, `refresh_token`, `expires_at`, `scope`, `token_type`, and
`expires_in`. Its storage directory is mode `0700`; token files are mode
`0600`; writes are temp-file + `fsync` + rename.

Refresh is lazy rather than background. It occurs when remaining lifetime is
less than `max(300 seconds, expires_in / 2)`. Refresh retries up to three times
for transport errors and HTTP `429`, `500`, `502`, `503`, and `504`, with 1s
then 2s backoff. `401`, `403`, or `invalid_grant` means re-login; the CLI
persists a revoked tombstone to avoid retrying a dead refresh token. Device
login has a 15-minute local deadline, honors server `interval`, and permanently
adds five seconds after `slow_down`.

Sources: [token response validation](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/packages/oauth/src/oauth.ts#L29-L53),
[on-disk schema](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/packages/oauth/src/types.ts#L63-L92),
[file storage](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/packages/oauth/src/storage.ts#L1-L117),
[refresh timing](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/packages/oauth/src/oauth-manager.ts#L27-L35),
[refresh failure behaviour](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/packages/oauth/src/oauth-manager.ts#L303-L397), and
[device polling lifecycle](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/packages/oauth/src/oauth-manager.ts#L400-L463).

For quota-widget, keep this bundle in its existing secret store rather than
copying Kimi Code's plaintext file storage. Treat a refresh-token rejection as
an actionable re-authentication state, not a generic quota-fetch error.

## Usage endpoint and parsing

The canonical Kimi Code API base is `https://api.kimi.com/coding/v1`; usage is
therefore `GET /coding/v1/usages`. The official fetch adds only `Authorization`
and `Accept` (no `X-Msh-*` or `User-Agent`) and aborts after eight seconds.
Before calling it, the toolkit runs the normal token freshness check.

Observed payload and normalization:

```json
{
  "usage": { "used": "40", "limit": "1000", "resetTime": "2026-08-03T05:20:51Z" },
  "limits": [
    {
      "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
      "detail": { "used": "1", "limit": "100", "resetTime": "..." }
    }
  ],
  "boosterWallet": { "...": "..." }
}
```

`used` and `limit` accept either JSON numbers or numeric strings. The top-level
`usage` has no explicit window, which Kimi Code labels as a one-week limit.
Each `limits[]` entry gets its name from the outer item (or `detail.name`), its
window from `window`, and its values/reset from `detail`. Valid time units are
`TIME_UNIT_MINUTE`, `TIME_UNIT_HOUR`, `TIME_UNIT_DAY`, and `TIME_UNIT_WEEK`;
whole-minute durations are folded to hours, so 300 minutes becomes five hours.

The client separately parses a `boosterWallet` only when
`boosterWallet.balance.type == "BOOSTER"`: fixed-point `amount` and
`amountLeft` are divided by 1,000,000 to get cents, alongside monthly-limit and
monthly-used money fields. This is credits/billing data rather than a quota
window and should be represented separately if quota-widget exposes it.

Sources: [base and endpoint](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/packages/oauth/src/managed-usage.ts#L27-L47),
[schema comment](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/packages/oauth/src/managed-usage.ts#L1-L21),
[summary and window parsing](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/packages/oauth/src/managed-usage.ts#L156-L242),
[booster parsing](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/packages/oauth/src/managed-usage.ts#L87-L154),
[HTTP request](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/packages/oauth/src/managed-usage.ts#L284-L321), and
[fresh-token-before-fetch](https://github.com/MoonshotAI/kimi-code/blob/fa9865f2ee653133295992489554bb2db05a9db5/packages/oauth/src/toolkit.ts#L283-L305).

## Implementation implication

Moonshot's ordinary Open Platform API-key balance and this Kimi Code OAuth
quota API are distinct integrations. A Moonshot provider should support a
Kimi-Code-OAuth account mode whose secret is a complete refreshable token
bundle, fetch a fresh access token before `GET /usages`, and map the weekly
summary plus every returned limit window into the widget's snapshot model.
Do not assume an undocumented scope, endpoint stability, or that a non-Kimi
client identity is accepted until manual validation confirms it.
