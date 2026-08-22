# Security Policy / セキュリティポリシー

## Reporting a vulnerability / 脆弱性の報告

Please **do not open a public issue** for security vulnerabilities.
Report privately via **GitHub Security Advisories**: [Report a vulnerability](https://github.com/iKora128/necoder/security/advisories/new).
You can expect an acknowledgement within 7 days.

脆弱性は公開 issue ではなく、上のリンク（GitHub の非公開報告）からお願いします。7 日以内に一次返信します。

## Supported versions

Pre-1.0: only the **latest release** receives fixes. The app self-updates from GitHub Releases
(the updater verifies the Apple code signature with `spctl` before installing).

## Scope notes

- necoder spawns local processes you configure (the `claude` CLI and other ACP agents, `ssh`, shells in the integrated terminal). Agent file edits go through an explicit permission/diff-review flow by default.
- Remote SSH deploys a static `necoder-remote-server` binary to hosts you connect to (checksum-verified; scoped to the project root you open).
- There is **no telemetry**; the only phone-home is the release version check against `api.github.com`.
