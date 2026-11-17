# Listing pingtable in winget

Once listed, `winget install LorinczLLC.pingtable` downloads the release zip,
unpacks it and makes `pingtable` and `pingtable-cli` work in any terminal. It
is a listing in Microsoft's community repository,
[microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs); the files
stay on GitHub releases.

## Make the manifests

After a release is published:

```bash
scripts/winget-manifest.sh
```

writes three files to `dist/winget/manifests/l/LorinczLLC/pingtable/VERSION/`.
The zip in `dist/` must be the file attached to the release, because winget
checks its SHA-256. Give a version (`scripts/winget-manifest.sh 0.2.0`) for an
older release.

Check them on Windows:

```
winget validate --manifest <that folder>
```

It should answer "Manifest validation succeeded."

## Send them

1. Fork [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs).
2. Copy the version folder to the same path in the fork:
   `manifests/l/LorinczLLC/pingtable/VERSION/`.
3. Open a pull request titled `New package: LorinczLLC.pingtable version VERSION`
   (later ones: `New version: …`). One version to a pull request.
4. Sign Microsoft's contributor agreement when the bot asks; it is asked once.

Microsoft's bots then download the zip, scan it and try the install. A person
approves the first submission of a new package.

## Things to know

- **Send it after signing, if signing is coming.** The repository refuses
  download links that SmartScreen flags as low reputation, and scans the
  programs with several antivirus engines. Unsigned programs from a new
  publisher are the likeliest to be held up.
- **Never replace a release zip.** The manifest pins its SHA-256; a changed
  file breaks installs. Publish a new version instead.
- **It installs as a portable package.** The folder is put on the PATH; there
  is no Start menu entry. `pingtable` opens the app.
- The manifests use schema 1.28.0, which the repository asked for on
  17 Nov 2026. If it has moved on, change `schema` in the script.

## Not tried yet

The 0.2.0 manifests pass `winget validate`. Nothing has been sent, and an
install through winget has not been tried.
