# Security policy

PrintNest's promise: **it opens files, edits them and writes files. It has no
other way to touch your data or the outside world.** It has no network code,
no telemetry, no accounts and no auto-updater.

## Reporting a vulnerability

Please do **not** open a public issue for security problems.

Use GitHub's private reporting: **Security → Report a vulnerability** on this
repository. Include the version, OS, steps to reproduce and, if possible, a
sample file that triggers the problem.

| Step | Target |
| --- | --- |
| Acknowledge your report | within 72 hours |
| Fix for high/critical issues | within 30 days |
| Public advisory | when a fixed release is out |

We credit reporters in the advisory unless you ask us not to.

## In scope

- Crashes, hangs or memory blow-ups from opening an image or project file
- Any way a file can make the app read/write outside folders you picked
- Any network traffic from the app
- Exported files leaking metadata (GPS, device info, usernames)
- Build, CI or release pipeline weaknesses (e.g. a way to get unreviewed code
  into a release)

## Supported versions

Only the latest release receives security fixes until 1.0.

## Verifying a download

Releases are not OS code-signed (no paid certificates), so verify before
running:

```sh
# 1. Check the file matches the published checksum
sha256sum -c SHA256SUMS --ignore-missing

# 2. Check it was built by this repo's release workflow
gh attestation verify <downloaded-file> --repo Obamna1234/printnest
```

Only download from this repository's GitHub Releases page (and Flathub for
Linux). Any other download site is not official.
