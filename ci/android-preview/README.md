# Android preview signing key

`debug.keystore` here is an **intentionally public, throwaway Android debug
keystore** — a PKCS#12 store with the standard debug credentials:

| field          | value                          |
| -------------- | ------------------------------ |
| store password | `android`                      |
| key alias      | `androiddebugkey`              |
| key password   | `android`                      |
| certificate    | `CN=Android Debug, O=Android, C=US` |

## Why it is committed

The `android-preview` workflow copies this file over the CI runner's
`~/.android/debug.keystore` before `tauri android build --debug`, so every
preview APK is signed with the **same** certificate. Android refuses an
in-place update when the signing certificate changes, and CI otherwise
generates a fresh random debug key on every run — which would break Obtainium's
update flow (you'd have to uninstall and reinstall each time). Pinning one key
here keeps updates working.

## This is not a secret

A debug keystore is not sensitive: the password is the well-known `android`, and
the only thing signing with it proves is "this is a debug build". It is **not**
the release signing key (there isn't one yet — see `docs/adr/0006-…`). The
preview APK it produces is debuggable and is **not** a release. Do not use this
key, or this channel, for anything you would actually ship.

Regenerated (if ever needed) with:

```sh
openssl req -x509 -newkey rsa:2048 -keyout key.pem -out cert.pem -days 10000 -nodes \
  -subj "/CN=Android Debug/O=Android/C=US"
openssl pkcs12 -export -in cert.pem -inkey key.pem -name androiddebugkey \
  -out debug.keystore -passout pass:android
```

Note: regenerating it changes the certificate, so anyone with a preview build
installed would need to uninstall and reinstall once after the change.
