# Security Policy

[日本語の案内は下にあります](#日本語)

## Supported versions

vw is maintained by individuals in their spare time, so only the **latest release** receives security fixes.
Older versions are not patched; please update to the latest release.

| Version | Supported |
| --- | --- |
| Latest release | Yes |
| Older releases | No |
| "最新ビルド (main)" (main-latest pre-release) | Fixed by the next build of main |

## Reporting a vulnerability

Please **do not open a public issue** for a security problem.
Report it privately through GitHub instead:

1. Open <https://github.com/xex-ken-xex/vw/security/advisories/new>
   (or the **Security** tab → **Report a vulnerability**)
2. Describe the problem, the affected version, and how to reproduce it (a sample file helps)

The report stays visible only to you and the maintainers until a fix is published.

Things worth reporting include, for example:

- A crafted image, archive, or other file that makes vw crash in a way that could be exploited, read or write outside its folders, or run code
- A problem with how vw downloads AI models or other files
- A compromised dependency, GitHub Action, or release file

## What happens after you report

1. We confirm the report and identify the affected versions. Expect a first reply within a few days.
2. We take down the affected releases (including the main-latest pre-release) or add a warning to them.
3. We pin or replace the cause (for example a dependency) and publish a fixed release.
4. We announce it in a GitHub Security Advisory (with a CVE if needed) and point to it from the README.
5. We let you know, and with your permission credit you in the advisory.

## Verifying a download

Each release has a `SHA256SUMS.txt` with the SHA-256 of its files, and the files carry a
GitHub artifact attestation that records which workflow run and commit built them.

```sh
# Linux
sha256sum -c SHA256SUMS.txt --ignore-missing
# Windows (PowerShell): compare with the line in SHA256SUMS.txt
Get-FileHash vw-windows-x64.zip -Algorithm SHA256
# Either, with the GitHub CLI: proves the file was built by this repository's CI
gh attestation verify vw-windows-x64.zip --repo xex-ken-xex/vw
```

---

## 日本語

セキュリティの問題（脆弱性）は、**公開の Issue に書かず**、GitHub の非公開の報告で知らせてください。

1. <https://github.com/xex-ken-xex/vw/security/advisories/new>
   （リポジトリの **Security** タブ → **Report a vulnerability**）を開きます
2. 問題の内容、版、再現の手順を書きます（再現するファイルがあると助かります）

報告は、修正を公開するまで報告者とメンテナだけが見られます。

- 直すのは **最新の版だけ** です。古い版を使っている場合は、最新の版に上げてください
- 受け付けたら確認して、数日以内を目安に返事をします。そのあと、影響する版の取り下げまたは注意書き、修正版の公開、
  GitHub Security Advisory での告知（必要なら CVE）を行い、了承があれば報告者の名前を謝辞に載せます
- ダウンロードしたファイルは、Release の `SHA256SUMS.txt`（SHA-256 の一覧）と、GitHub の artifact attestation
  （どの CI のどのコミットで作ったかの証明。`gh attestation verify ファイル --repo xex-ken-xex/vw`）で確かめられます
