# The Microsoft Store build

The Store build is the same two programs in an MSIX package. Microsoft signs
it, installs it, and updates it, so it never shows an "unknown publisher"
warning. It is the paid edition: the zip on GitHub stays free.

## Once: Partner Center

1. **Register the company** in
   [Partner Center](https://partner.microsoft.com/dashboard). Company
   registration has been free since May 2026; the company still has to pass
   business verification.
2. **Reserve the name** "pingtable" by starting a new app.
3. **Copy the identity values** from the app's Product identity page into a
   file named `store.env` beside `Cargo.toml` (ignored by git):

   ```
   PINGTABLE_STORE_NAME=<Package/Identity/Name>
   PINGTABLE_STORE_PUBLISHER=<Package/Identity/Publisher, starting CN=>
   PINGTABLE_STORE_PUBLISHER_NAME=<Package/Properties/PublisherDisplayName>
   ```

## Every release

```bash
scripts/package-msix.sh
```

builds the programs and packs `dist/pingtable-VERSION-x64.msix`. It needs
`makeappx.exe` from the Windows SDK, so run it on a Windows machine (from WSL
or Git Bash). Upload the file in Partner Center as a new submission. Do not
sign it yourself: the Store replaces the signature with Microsoft's.

Without `store.env` the script packs a preview with a made-up identity. That
proves the package builds; it cannot be submitted.

## In the submission

- **Price:** set the base price there. The plan is $14.99, paid once.
  Microsoft keeps 15% of app sales.
- **`runFullTrust`:** the package asks for this restricted capability, as
  every packaged desktop program does. Partner Center asks why. The answer:
  pingtable is a classic Win32 program that sends ICMP pings through the
  Windows IP Helper API and shows its interface in a WebView2 window.
- **Privacy:** Partner Center is likely to ask for a privacy policy link,
  because the package can use the network. The honest text is short:
  pingtable collects nothing and contacts only the hosts you list.
- **Description and pictures:** the README and the site have the words;
  screenshots have to be taken from the app.

## What is in the package

`packaging/msix/AppxManifest.xml` describes it. `pingtable.exe` is the app
with its tile. `pingtable-cli.exe` has no tile and is offered as a command,
so `pingtable-cli` works in any terminal, including as an MCP server for an
AI agent. The tiles are drawn from the app's icon when the package is made.

Settings live in the per-user folder, as with the zip; a `pingtable.json`
beside the program is not possible inside a package.

## Not tried yet

The preview package builds and passes the packing tool's checks. It has not
been installed, because installing an unsigned package needs developer mode
or a trusted certificate, and it has not been through Store certification.
Try the first real package on your own machine through a private Store
listing before making it public.
