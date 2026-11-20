# Releasing

Design §10: GitHub Actions → Releases for Windows (unsigned), macOS (signed + notarized) and Android (APK);
iOS through TestFlight, for the owner only.

## Desktop and Android: GitHub release

1. Set the same version in `desktop/src-tauri/tauri.conf.json`, `mobile/app.json` (`expo.version`) and the
   Cargo manifests. Commit.
2. Tag and push: `git tag -a v0.2.0 -m "kitedrop 0.2.0"` then `git push origin v0.2.0`. A tag with a suffix
   (`v0.2.0-beta.1`) becomes a pre-release. The `check` job stops the run if the tag and the versions differ.
3. [.github/workflows/release.yml](../.github/workflows/release.yml) builds on GitHub's runners:

   | Job | Output |
   |---|---|
   | windows | `kitedrop-<tag>-windows-x64-setup.exe` (NSIS, per-machine, firewall rule + "Send to") |
   | macos | `kitedrop-<tag>-macos-universal.dmg` and `.zip` (Apple silicon + Intel) |
   | android | `kitedrop-<tag>-android-arm64.apk` |

   and creates a **draft** release with `SHA256SUMS.txt`. Review it, then publish it on GitHub.
4. A rebuild of an existing tag: *Actions → release → Run workflow* with the tag.

### macOS from a Mac (until the Apple secrets are in GitHub)

Without the Apple secrets the workflow's macOS build is only ad-hoc signed. Build the macOS files on a Mac with
the Developer ID certificate and a notarytool profile in the login keychain, then replace the two macOS files of
the draft release (and their lines in `SHA256SUMS.txt`):

```sh
APPLE_SIGNING_IDENTITY="Developer ID Application: NAME (TEAMID)" NOTARY_PROFILE=<profile> scripts/release-mac.sh
```

It builds a universal app with the share extension, notarizes and staples the app and the `.dmg`, and prints
Gatekeeper's verdict and the checksums. The profile is created once with
`xcrun notarytool store-credentials <profile> --apple-id <apple id> --team-id <team id>`.

### Secrets (repository settings → Secrets and variables → Actions)

Every secret is optional; the build runs without them and the job log warns about what is missing.

| Secret | Used for | Without it |
|---|---|---|
| `APPLE_CERTIFICATE` | base64 of the *Developer ID Application* certificate exported as .p12 (`base64 -i cert.p12 \| pbcopy`) | app is ad-hoc signed; macOS Local Network permission does not work reliably (D33) |
| `APPLE_CERTIFICATE_PASSWORD` | password chosen when exporting the .p12 | — |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: NAME (TEAMID)` | — |
| `APPLE_API_ISSUER`, `APPLE_API_KEY`, `APPLE_API_PRIVATE_KEY` | notarization: App Store Connect → Users and Access → Integrations → App Store Connect API → a *Developer* key: issuer id, key id, contents of the downloaded `AuthKey_<id>.p8` | not notarized; Gatekeeper blocks the first open (right-click → Open) |
| `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD` | release signing key for the APK | APK keeps the Expo template's public debug signature (see SECURITY.md) |

Tauri reads the Apple variables itself (Tauri docs, "macOS Code Signing": `APPLE_CERTIFICATE`,
`APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`; notarization with `APPLE_API_ISSUER`, `APPLE_API_KEY`,
`APPLE_API_KEY_PATH`). The workflow writes the .p8 to the runner's temp folder and deletes it after the build.

**Switching the Android signature** (debug → release key) changes the app's identity for Android: a phone with
the old build must uninstall it first, which deletes its kitedrop identity, so the phone has to be removed from
the group and paired again. Decide on the key before giving the APK to anyone.

To make a release keystore (keep it and its passwords outside the repository, e.g. in a password manager):

```sh
keytool -genkeypair -v -keystore kitedrop-release.jks -alias kitedrop -keyalg RSA -keysize 4096 -validity 10000
base64 -i kitedrop-release.jks | pbcopy     # → ANDROID_KEYSTORE_BASE64
```

## iPhone: TestFlight

TestFlight is for the owner's own devices (internal testing). Nothing secret is stored: the upload uses the
Apple account signed in to Xcode.

One-time setup (owner, in a browser — not automated):

1. [Apple Developer → Identifiers](https://developer.apple.com/account/resources/identifiers/list): the App IDs
   `com.mucahitimre.kitedrop` and `com.mucahitimre.kitedrop.share`, both with the **App Groups** capability
   and the group `group.com.mucahitimre.kitedrop`. (Xcode's automatic signing creates these during development
   builds; check that they exist.)
2. [App Store Connect → Apps → +](https://appstoreconnect.apple.com/apps): a new iOS app with bundle id
   `com.mucahitimre.kitedrop`, any name that is still free, SKU e.g. `kitedrop`.
3. In Xcode → Settings → Accounts, the Apple account of team `4SFTG4T5YD` is signed in.

Each build:

```sh
scripts/testflight.sh            # archive + export build/testflight/kitedrop.ipa (nothing is uploaded)
scripts/testflight.sh --upload   # archive + upload to App Store Connect
```

The export is marked *TestFlight internal testing only* and Xcode sets the build number. After processing,
the build appears in App Store Connect → TestFlight; add yourself as an internal tester and install it with the
TestFlight app.

Open points (not verified yet, they need the App Store Connect app record):

- **Export compliance.** kitedrop encrypts transfers (HPKE / ChaCha20-Poly1305), so App Store Connect asks
  about encryption for each build. The answer is the owner's legal call; `ITSAppUsesNonExemptEncryption` is
  deliberately not set in `app.json` until then.
- **Push notifications** (design §9) need the Push Notifications capability and an APNs key in the owner's
  Expo account; not set up yet.
