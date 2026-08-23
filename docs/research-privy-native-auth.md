# Privy documentation and a native Hermes sign-in

Research date: 2026-08-19. Sources below are Privy's official documentation.

## Finding

Privy does document authentication for applications that *own a Privy app*: it
has client SDKs for web, React Native, Swift/iOS and Android, and its native
clients can perform methods such as an SMS one-time-passcode login. That is not
a published, general OAuth/OIDC authorization-server contract for an unrelated
desktop client to reproduce another product's login.

The documented integration is application-bound. The application developer
creates/configures an app and app clients in the Privy Dashboard, supplies that
app's `appId` (and optionally its app-client ID) to Privy's SDK, and configures
allowed app identifiers and callback URL schemes for non-web clients. Privy's
docs say these are required for non-web/mobile API interaction and that an empty
allowed-identifier or URL-scheme list rejects requests/redirects. Consequently,
quota-widget cannot safely add a direct Nous/Hermes sign-in merely because
Nous uses Privy: it would need Nous to provide and authorize a suitable Privy
app/client configuration (and a supported desktop integration), or a
Nous-supported OAuth/API flow.

The public token documentation points in the same direction: access tokens are
JWTs whose audience must match the Privy app ID; refresh tokens are opaque,
SDK-managed, inaccessible to developers, rotated by Privy, and explicitly not
to be manually managed or shared across applications. Reusing Hermes's client
identity or refresh material would therefore be both unsupported and unsafe.

## What the documentation does cover

- [App clients](https://docs.privy.io/basics/get-started/dashboard/app-clients)
  describes web/mobile/other clients. Non-web clients require allowed app
  identifiers; social-login redirects require registered URL schemes. Both are
  configured by the Privy app owner in its Dashboard.
- [React setup](https://docs.privy.io/basics/react/setup) requires the
  developer's Privy app ID and optionally an app-client ID. [Swift
  quickstart](https://docs.privy.io/basics/swift/quickstart) shows native SDK
  login (SMS OTP) after the app owner enables that login method in the
  Dashboard.
- [Tokens](https://docs.privy.io/authentication/user-authentication/tokens)
  states that access tokens include the app ID, refresh tokens are managed by
  Privy SDKs and unavailable to developers, and warns against manual refresh
  token management or cross-application token sharing.
- [Custom OAuth providers](https://docs.privy.io/authentication/user-authentication/login-methods/custom-oauth)
  is about a Privy app owner configuring an *upstream* OAuth identity provider
  in that owner's Dashboard (including its client secret, callback URL, and
  optional PKCE). It does not describe Privy itself exposing generic
  authorization/token/device endpoints to third-party relying applications.

## Implication for Hermes

The existing adapter's conservative design remains justified: consume the
short-lived access token that `hermes-agent` owns and let that client manage
its session. A native widget sign-in is possible only as a coordinated Nous
integration, not by treating Privy's developer documentation as permission to
emulate Hermes. Before designing it, obtain an official Nous contract covering
the authorized client/app identity, supported Windows/Linux callback approach,
available login methods, token audience accepted by the billing API, and the
token-refresh ownership/rotation rules.
