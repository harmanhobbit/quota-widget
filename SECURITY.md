# Security Policy

Quota Widget is a desktop tray widget that reads AI-provider quotas. It stores
provider credentials locally — DPAPI-backed on Windows, a `0600` plaintext file
on Linux — so a defect that leaks or mishandles those secrets is treated as a
security issue, not an ordinary bug.

## Supported versions

Only the latest released version receives security fixes. Releases are
[Semantic Versioning](https://semver.org); fixes ship in a new PATCH (or the
next release if one is already in flight) rather than as backports to older
lines. Check the version in the widget's Settings against the
[latest release](https://github.com/harmanhobbit/quota-widget/releases) before
reporting.

## Reporting a vulnerability

**Please report privately — do not open a public issue for a suspected
vulnerability.**

Use GitHub's private vulnerability reporting:

1. Go to the repository's **Security** tab, or open
   <https://github.com/harmanhobbit/quota-widget/security/advisories/new>.
2. Click **Report a vulnerability** and describe the issue.

This opens a private security advisory visible only to you and the repository
maintainers. If you cannot use GitHub advisories, open a minimal public issue
that says only that you have a security report and asks for a private channel —
do not include details there.

A useful report includes:

- affected version and platform (Windows 11 or Linux, and the desktop if
  relevant);
- what the issue is and its impact (for example, credential exposure, a write
  outside the config directory, or a network request to an unexpected host);
- steps or a proof of concept to reproduce it;
- any suggested fix.

## What to expect

This is a small, maintainer-driven project, so responses are best-effort rather
than bound to a fixed SLA. Please allow time for a maintainer to acknowledge and
investigate before disclosing publicly. A fix will be released as a new version,
and we will credit reporters who want it once the advisory is resolved.

## Scope

In scope: the application source in this repository and the code paths that
handle credentials, secret storage, configuration files, in-app updates, and
outbound provider requests.

Out of scope: vulnerabilities in third-party provider APIs or in upstream
dependencies (report those to the respective project), and issues that require
an already-compromised local account — the Linux secret file's `0600` mode
protects against *other* local users, not against the account that owns it.
