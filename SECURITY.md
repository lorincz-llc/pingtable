# Security

## Report a suspected vulnerability privately

Email [support@pingtable.net](mailto:support@pingtable.net). Include the pingtable version, operating system, affected component, impact and a short description of what you observed. Use documentation addresses or sanitized examples. Keep credentials, API tokens, personal information, internal host lists, configuration files and packet captures out of the initial report and public issues.

The project reviews reports for the current release. A confirmed issue may result in a patch or advisory. No response deadline or bounty is promised.

## Local access and commands

The HTTP interface listens on loopback. The server checks the Host header and any supplied Origin. State and control requests require the bearer token from the configuration file. The local discovery handshake uses a nonce and keyed proof so pingtable's clients can identify a running instance before presenting the token.

Treat the configuration, API token and browser launch URL as credentials. Someone with access to your account or token may be able to read network information, change monitored destinations or configure an alert command. Protect both the program directory and configuration with your account's permissions. Back up your settings securely. If the token was exposed, stop the app, set the `token` field in the configuration to an empty string and restart. The app generates and saves a fresh token. Update any clients that stored the old one.

Alert commands run as the user who started pingtable. Keep them disabled until needed. To manage commands exclusively through the local file, set `"commands_locked": true` in `pingtable.json`. Limit write access to that file and to any script it invokes. Treat exact values passed through `PINGTABLE_` environment variables as untrusted data in your script. Use ordinary account privileges.

The agent interface offers a read-only view of the table by default. An explicit `--allow-add` enables limited additions. One-shot diagnostic tools can generate network traffic. Give agent tools only to users you trust with the selected destinations. On shared WSL systems, use `--no-open` to avoid passing the browser launch URL to Windows through a process argument. [Agent guide](docs/ai-agents.md)

## Releases

Download packages from the project's GitHub releases. Compare the ZIP's SHA-256 with the digest in that release. For a build described as signed, inspect both executable signatures and their publisher in Windows. For a suspected altered package, include its filename, release version and computed digest in the private report. [Releases](https://github.com/lorincz-llc/pingtable/releases) · [Build and signature verification](docs/signing.md)
