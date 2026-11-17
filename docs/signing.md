# Signing the Windows programs

Signed programs show a publisher name instead of "unknown publisher", and an
IT department can allow that publisher. This page is for whoever makes the
releases. Facts about Microsoft's service were checked on 17 Nov 2026 and
change; the links are the authority.

`scripts/sign-windows.sh` signs `pingtable.exe` and `pingtable-cli.exe` with
[Microsoft Artifact Signing](https://azure.microsoft.com/en-us/products/artifact-signing)
(formerly Trusted Signing) through [jsign](https://ebourg.github.io/jsign/),
which runs on Linux as well as Windows. `scripts/package-windows.sh` calls it
by itself once the three settings below exist. Until then it packages
unsigned programs and says so in the zip and in the release notes.

## Once: set up the service

Microsoft's own walk-through is the
[quickstart](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart).
In short:

1. **An Azure subscription** that can be billed. The Basic plan is about
   $9.99 a month.
2. **An Artifact Signing account**, in one region. Note the region's
   endpoint from the table below.
3. **Identity validation for the organisation.** Microsoft checks the
   company: a website and mailboxes on a company-owned domain, a business
   identifier, and an identity check on a person. It can take from a day to
   several weeks. Microsoft's pages disagree on whether a company needs
   three years of history, so a young company may be refused.
4. **A certificate profile** of the Public Trust kind, using that validated
   identity. The name on the profile is the name Windows will show.
5. **Permission to sign.** The account that signs needs the role
   "Artifact Signing Certificate Profile Signer" on the profile
   ([how](https://learn.microsoft.com/en-us/azure/artifact-signing/tutorial-assign-roles)).

The endpoint must be the region the account and the profile were created in;
another one answers 403.

| Region | Endpoint |
|---|---|
| East US | `eus.codesigning.azure.net` |
| Central US | `cus.codesigning.azure.net` |
| North Central US | `ncus.codesigning.azure.net` |
| South Central US | `scus.codesigning.azure.net` |
| West Central US | `wcus.codesigning.azure.net` |
| West US | `wus.codesigning.azure.net` |
| West US 2 | `wus2.codesigning.azure.net` |
| West US 3 | `wus3.codesigning.azure.net` |
| North Europe | `neu.codesigning.azure.net` |
| West Europe | `weu.codesigning.azure.net` |
| Poland Central | `plc.codesigning.azure.net` |
| Switzerland North | `swn.codesigning.azure.net` |
| Brazil South | `brs.codesigning.azure.net` |
| Japan East | `jpe.codesigning.azure.net` |
| Korea Central | `krc.codesigning.azure.net` |

([Microsoft's list](https://learn.microsoft.com/en-us/azure/artifact-signing/how-to-signing-integrations))

## Once: set up the build machine

1. **Java 8 or newer.**
2. **jsign.** Download `jsign-7.6.jar` (2.5 MB) from its
   [releases page](https://github.com/ebourg/jsign/releases/tag/7.6). GitHub
   lists its SHA-256 as
   `6cf58aa080e7ff82656f320cff8d4a3bff30408df8766806dcba3740dd839fa5`;
   compare before using it.
3. **The Azure CLI**, signed in with `az login` as the account that has the
   signer role. The script asks it for a short-lived token each time.
4. **A file named `signing.env`** beside `Cargo.toml`. It is ignored by git
   and holds no secret:

   ```
   PINGTABLE_SIGN_ENDPOINT=eus.codesigning.azure.net
   PINGTABLE_SIGN_ACCOUNT=your-account-name
   PINGTABLE_SIGN_PROFILE=your-profile-name
   JSIGN_JAR=/path/to/jsign-7.6.jar
   ```

   `JAVA=/path/to/java` as well, if `java` is not on the PATH.

Without the Azure CLI, set `PINGTABLE_SIGN_TOKEN` to an access token, or set
`AZURE_TENANT_ID`, `AZURE_CLIENT_ID` and `AZURE_CLIENT_SECRET` for a service
principal and jsign signs in by itself.

## Every release

```bash
scripts/package-windows.sh
```

builds, signs, checks and packages. On a Windows machine (WSL included) the
script asks Windows whether each signature is valid and stops if one is not.
To check at any time:

```bash
scripts/sign-windows.sh --verify
```

On Windows itself: `Get-AuthenticodeSignature .\pingtable.exe` should say
`Valid` and name the publisher.

`WebView2Loader.dll` is Microsoft's file and is left as it is.

## What signing does and does not do

- Windows shows the publisher's name, and IT can allow the publisher. The
  service issues short-lived certificates that change every few days, so an
  allow rule should name the publisher, not one certificate.
- The SmartScreen warning does not disappear on the first day. Reputation
  builds with downloads, per publisher, and carries over to later releases
  signed by the same publisher
  ([Microsoft on SmartScreen](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation)).
- The service adds a trusted timestamp, so a signature stays valid after its
  certificate expires.

## Not tried yet

The script has been run against stand-in tools to check what it asks jsign
to do, and its checks have been run on unsigned programs. It has not signed
with the real service, which needs the account above.
