# The Microsoft Store build

The Microsoft Store package contains the same free app and command-line program as
the native-window ZIP. This page describes how to prepare a Store submission.

## Once: Partner Center

1. **Register the company** in
   [Partner Center](https://partner.microsoft.com/dashboard). Complete the
   business verification requested for the company account.
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
scripts/package-msix.sh --no-build
```

packs `dist/pingtable-VERSION-x64.msix`. Cross-build with
`scripts/build-windows.sh` on Linux first. Copy the entire `dist/windows`
directory to the same source revision on native Windows. It includes the
loader, the matching toolchain notices and their checksum manifest. Run
the packer in Git Bash with the Windows SDK, then upload the unsigned MSIX
in Partner Center for Store signing.

Without `store.env` the script packs a preview with a made-up identity. That
proves the package builds; it cannot be submitted.

## In the submission

- Set the listing price to **Free**.
- **`runFullTrust`:** the package asks for this restricted capability, as
  every packaged desktop program does. Partner Center asks why. The answer:
  pingtable is a classic Win32 program that sends ICMP pings through the
  Windows IP Helper API and shows its interface in a WebView2 window.
- **Privacy:** Partner Center is likely to ask for a privacy policy link,
  because the package can use the network. The honest text is short:
  settings and hosts are stored locally. Probes contact the selected hosts
  and use the system’s address and name resolution.
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

The 0.4.0 package has not been installed or submitted for Store certification.
Verify installation, launch and the command-line alias on native Windows
through a private listing before a public submission.
