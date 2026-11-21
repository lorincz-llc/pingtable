# Windows releases and signing

Pingtable publishes two free Windows ZIPs from a version tag. Both carry the full table, CLI, local API and MCP server. The native edition includes the WebView2 desktop window. The browser edition runs the table in the default browser. The current GitHub workflow produces unsigned Pingtable executables and checks that the release notes say so. The public release also carries GitHub build provenance for both ZIPs. Windows can display “unknown publisher” until the project obtains a trusted Authenticode signature.

## Publish a release

1. Set the workspace version in `Cargo.toml`, update `Cargo.lock`, and add the matching version section to `CHANGELOG.md`. Refresh third-party notices after changes to dependencies, bundled fonts or the Windows loader.
2. Commit the reviewed changes and push that commit to the release branch. Create an annotated tag on the intended commit and push only that tag. The 0.4.0 release tag is `v0.4.0+build.1`. A build revision preserves earlier tags if validation requires changes before the first publication.
3. Watch **Actions → Release** for the tag. Ubuntu tests the tagged sources, cross-builds both editions with MinGW, checks their SHA-256 digests and records build provenance. A Windows runner opens each archive, checks the digest and Authenticode status, runs each packaged CLI with `--version`, and runs the workspace tests and Clippy.
4. The publish job takes the exact artifact tested on Windows. It checks the assets in a draft, then publishes the generated release notes, both ZIPs, both individual checksum files and `SHA256SUMS`. Use the published ZIPs for any package-manager submission.

The workflow pins its actions to upstream commit SHAs. When upgrading an action, review the upstream release, resolve its version tag to the commit, and update both the SHA and its version comment. Keep the tested Rust version consistent with CI. The packaging step needs the Rust documentation component for the toolchain license files. [Release workflow](../.github/workflows/release.yml) · [GitHub Actions security guidance](https://docs.github.com/en/actions/reference/security/secure-use)

### Recover an interrupted release

Use **Re-run failed jobs** on the original tagged run. The publish job downloads the build artifact by ID. It fills gaps in a draft and compares existing assets byte for byte. If a draft contains different bytes, inspect the draft and job logs before deciding which build to keep. When the tag is already published, the job downloads and verifies its published checksums and leaves the release intact. Ship a new patch version when published files need a correction. The Actions artifact is retained for 30 days. [GitHub release protection](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases)

### Verify a download

Run these commands in the directory containing both ZIPs and the release’s `SHA256SUMS`. Adjust the version when checking another release.

```bash
sha256sum --check SHA256SUMS
gh attestation verify pingtable-0.4.0-windows-x64.zip --repo lorincz-llc/pingtable --source-ref refs/tags/v0.4.0+build.1 --signer-workflow lorincz-llc/pingtable/.github/workflows/release.yml
gh attestation verify pingtable-0.4.0-browser-windows-x64.zip --repo lorincz-llc/pingtable --source-ref refs/tags/v0.4.0+build.1 --signer-workflow lorincz-llc/pingtable/.github/workflows/release.yml
```

The digest checks the downloaded bytes. Attestation verification also checks the repository, workflow and source tag. Authenticode separately identifies the executable’s publisher to Windows. GitHub supports public-repository attestations. Private repositories on a plan with attestation support can set `ENABLE_PRIVATE_ATTESTATIONS=true` to enable the same workflow step. [Artifact attestations](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations) · [GitHub CLI verification options](https://cli.github.com/manual/gh_attestation_verify)

## Microsoft Artifact Signing

Use a **Public Trust** certificate profile if Windows should show the project’s validated organization as the publisher. The owner needs an Azure subscription, a supported signing region, a signing account, approved organizational identity validation and a certificate profile tied to that identity. Assign the **Artifact Signing Certificate Profile Signer** role at the profile scope to the identity doing the signing. Check Microsoft’s current eligibility requirements, pricing and regional endpoint before enrolling. [Microsoft quickstart](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart) · [Role assignment](https://learn.microsoft.com/en-us/azure/artifact-signing/tutorial-assign-roles) · [Pricing](https://azure.microsoft.com/en-us/pricing/details/artifact-signing/)

The local signing script supports Microsoft’s service through Jsign. Install Java, an approved Jsign release and Azure CLI, then sign in using the identity with the signer role. Verify the Jsign download against its upstream release digest. An ignored `signing.env` alongside `Cargo.toml` can hold the following configuration. Replace the endpoint with the region assigned to the account. [Jsign releases](https://github.com/ebourg/jsign/releases) · [Microsoft signing integrations and endpoints](https://learn.microsoft.com/en-us/azure/artifact-signing/how-to-signing-integrations)

```text
PINGTABLE_SIGN_ENDPOINT=eus.codesigning.azure.net
PINGTABLE_SIGN_ACCOUNT=your-account-name
PINGTABLE_SIGN_PROFILE=your-public-trust-profile
JSIGN_JAR=/path/to/jsign.jar
```

On a configured build machine, `scripts/package-windows.sh` builds the executables, runs `scripts/sign-windows.sh` and packages the signed files. The script requests a timestamp for both executables. Keep access tokens and credentials out of the repository. Microsoft’s redistributable loader ships with its vendor license and its original bytes. [Signing script](../scripts/sign-windows.sh)

Before enabling signing in the tag workflow, configure the protected signing identity, pass the verified build artifacts to the signer, and package the signed outputs before generating any checksums or provenance. Update the Windows gate to require `Get-AuthenticodeSignature` status `Valid`, the expected publisher, a timestamp, and matching wording in the release notes. Confirm the result on a native Windows runner. The current workflow requires `NotSigned` for the Pingtable executables.

## SignPath Foundation for open source

SignPath Foundation offers eligible open-source projects a signing service after application and review. If accepted with the Foundation certificate, Windows identifies **SignPath Foundation** as the publisher. Apply with the public repository, release history and the project’s code-signing policy. Review eligibility of the bundled Microsoft WebView2 loader with the Foundation. Restrict a signing request to the project-built executables and distribute the upstream DLL unchanged. [Eligibility and conditions](https://signpath.org/terms) · [Application](https://signpath.org/apply)

The integration should use a reviewed GitHub Actions build artifact, the project’s approved artifact configuration and its designated release approvers. Package the returned signed executables and update the publisher name in the package readmes, release notes and Windows verification gate to match the issuing certificate. The existing Azure signing script targets Microsoft Artifact Signing. A SignPath release needs its own approved signing request in the workflow. [SignPath’s GitHub integration](https://docs.signpath.io/trusted-build-systems/github)

## Check Authenticode on Windows

```powershell
Get-AuthenticodeSignature .\pingtable.exe, .\pingtable-cli.exe |
    Select-Object Path, Status, SignerCertificate, TimeStamperCertificate
```

A valid signature allows Windows and management tools to identify the publisher. SmartScreen also considers reputation and may prompt on newly released files. [Microsoft on SmartScreen reputation](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation)
