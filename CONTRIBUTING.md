# Contributing

pingtable is open source and under active development by Lorincz LLC. Bug
reports and ideas are welcome as
[issues](https://github.com/lorincz-llc/pingtable/issues). Security problems
go by email instead: see [SECURITY.md](SECURITY.md).

Pull requests are welcome too. They are reviewed as time allows, and a change
that does not fit the direction of the project may be declined.

## Before a pull request

```bash
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

- Keep a change small and about one thing.
- The interface in `ui/` is plain HTML, CSS and JavaScript with no build step.
  Keep it that way, and keep both looks (light and dark) working.
- pingtable talks only to the hosts in the list and to the system's name
  resolver. Do not add telemetry, update checks or any other destination.
- It must keep running without administrator rights.
- Use made-up examples in code, tests and documents: `example.com`, names
  ending in `.lan` or `.example`, and private or documentation address ranges.
  No real host names, addresses or personal details.
- Add a line to [CHANGELOG.md](CHANGELOG.md) for anything a user would notice.

## Maintaining releases

Dependabot opens weekly update PRs for Cargo and GitHub Actions. Review each change and its upstream release notes. After a dependency update, regenerate the Windows notices with `python3 scripts/third-party.py`, review the attribution changes, and commit the matching `Cargo.lock`. CI runs the locked workspace tests, Clippy and the RustSec audit.

If an advisory applies to a transitive dependency, check the active dependency tree for each supported target. Cargo can keep optional platform dependencies in the lockfile even when the release builds do not link them. Document the affected target and test the update before publishing. Follow the [release and signing guide](docs/signing.md) for tagged builds.

## Whose work it is

By sending a contribution you agree that it is licensed under the
[Apache License, Version 2.0](LICENSE), as section 5 of the license describes.
There is no separate contributor agreement.

Send only work you have the right to send. Do not copy code, text, artwork or
other material from any other product, including other ping tools, unless its
license allows it and you say where it came from.
