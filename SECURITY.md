# Security

## Reporting a problem

Email **pingtable@lorincz.llc**. Please do not open a public issue for a
security problem.

Say which version you have (the About box, or `pingtable-cli --version`), what
you did and what happened. A short way to reproduce it helps most.

Fixes go into the next release; only the latest release is maintained. There
is no bug bounty and no promised response time (see
[DISCLAIMER.md](DISCLAIMER.md)).

## What pingtable does and does not do

- It talks only to the hosts in your list and to the system's name resolver.
  It has no telemetry, no update check and no account.
- Its interface is an HTTP server on the loopback address only. Every API call
  needs the access token kept in the config file. Requests whose `Host` or
  `Origin` is not this machine are refused.
- The settings can hold commands that pingtable runs when a host goes down or
  comes back. Anyone who can read the config file, or who has the token, can
  therefore make pingtable run a command as you. Treat `pingtable.json` like a
  script: keep it where only you can write.
- Values substituted into alert commands (`%HostName%` and the rest) are
  reduced to plain characters first, so text from an imported file cannot add
  commands of its own.
- Pinging uses the unprivileged Windows ICMP API. pingtable needs no
  administrator rights and should not be run with them.

## Release files

The Windows programs are not code-signed yet. Each release lists the SHA-256
of its zip; compare it before running, or build from source
([README](README.md#building-for-windows-from-linux)).
