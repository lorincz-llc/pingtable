# Using pingtable from an AI agent

pingtable gives an agent six tools for answering "is it up, is the port open,
and where does the path stop". They run on this computer, need no account and
no internet connection of their own, and work with any model: a hosted one or
one running locally.

There are three ways in. Use the first one your agent supports.

| Way in | For | What the agent needs |
|---|---|---|
| [MCP server](#mcp-server) | Any agent or chat app that speaks the Model Context Protocol | The command `pingtable-cli mcp` |
| [Command line](#command-line) | An agent that can run shell commands | `pingtable-cli once` and `pingtable-cli ports` with `--format json` |
| [HTTP API](#http-api) | A script or agent that can make HTTP calls to this computer | The address and token from `pingtable-cli api` |

Whichever way is used, checks are sent **from this computer**. A result says
what this computer can reach, not what the rest of the world can.

## MCP server

`pingtable-cli mcp` is a Model Context Protocol server on standard input and
output. The client starts it; nothing has to be running beforehand and no
port is opened.

### Set it up

Use the full path to `pingtable-cli.exe` in the folder you unpacked. The
examples assume `C:\Tools\pingtable`.

**Claude Desktop.** Settings, Developer, Edit Config, then add to
`claude_desktop_config.json` and restart the app:

```json
{
  "mcpServers": {
    "pingtable": {
      "command": "C:\\Tools\\pingtable\\pingtable-cli.exe",
      "args": ["mcp"]
    }
  }
}
```

**Claude Code.**

```
claude mcp add pingtable -- "C:\Tools\pingtable\pingtable-cli.exe" mcp
```

**Any other client**, including the ones that run local models: add the same
`mcpServers` entry in that client's MCP settings. The shape above (a command
and its arguments, speaking over standard input and output) is the one MCP
clients share.

**From source or on Linux:** the command is `pingtable mcp`.

To point the two table tools at a pingtable that uses its own settings file,
add `"--config", "C:\\path\\pingtable.json"` to the arguments.

### What leaves this computer

pingtable sends pings and connection attempts to the hosts the agent names,
and nothing else to anyone. What the tools return goes to the model your
client uses. With a local model that stays on this computer; with a hosted
model the results, including host names and addresses on your network, are
sent to that provider like the rest of the conversation.

### The tools

| Tool | Answers | Changes anything? |
|---|---|---|
| `ping` | Do these hosts answer, and how fast? | No |
| `check_ports` | Is a TCP service listening on this host? | No |
| `trace_route` | Which routers are on the way, and where does it stop? | No |
| `network_info` | What is this computer's name and addresses? | No |
| `table_status` | What does the running pingtable app show right now? | No |
| `table_add_hosts` | Add hosts to the running app's table | Yes: the saved host list |

Every tool returns a short text summary and the same facts as structured
data. A failed call (a bad host name, too many ports) comes back as an
ordinary result marked as an error, with a sentence saying what to change.

#### ping

| Argument | Type | Meaning |
|---|---|---|
| `hosts` (required) | list of text | One host per entry: a name or address (ICMP ping), or `host:port` (a TCP connection, for hosts that block ICMP). `192.168.1.1-192.168.1.20` and `192.168.1.0/28` expand. At most 256 hosts after expanding |
| `count` | whole number, 1 to 10, default 1 | Pings per host. Use more than one to measure loss |
| `timeout_ms` | 50 to 10000, default 1000 | How long to wait for each answer |

Call:

```json
{ "name": "ping", "arguments": { "hosts": ["192.168.1.1", "example.com:443", "192.0.2.1"], "count": 3 } }
```

Text returned:

```
2 of 3 hosts answered, pinged from MY-PC.
- 192.168.1.1 icmp: up, 3 of 3 answered, average 0.42 ms, slowest 0.51 ms
- example.com (93.184.215.14) tcp:443: up, 3 of 3 answered, average 12.8 ms, slowest 13.4 ms
- 192.0.2.1 icmp: no answer (Timed out)
```

Structured data: `from`, `count`, `timeout_ms`, `answered`, `total`,
`complete`, `skipped`, and `hosts`, each with `host`, `ip`, `check` (`icmp` or
`tcp:PORT`), `up`, `status`, `sent`, `answered`, `loss_pct`, `last_ms`,
`avg_ms`, `min_ms`, `max_ms`.

Reading it: `up` means at least one ping was answered. **No answer does not
prove a host is off.** Many hosts and most firewalls drop pings; follow up
with `check_ports` on a port the host should serve.

#### check_ports

| Argument | Type | Meaning |
|---|---|---|
| `host` (required) | text | One host name or address |
| `ports` | list of whole numbers, at most 64 | Ports to check. Left out, the usual ones are checked: 22, 80, 443, 3389, 445, 53, 25, 3306, 5432, 1433, 8080, 8443 |
| `timeout_ms` | 200 to 10000, default 3000 | How long to wait for each port |

Call:

```json
{ "name": "check_ports", "arguments": { "host": "example.com", "ports": [22, 80, 443] } }
```

Text returned:

```
example.com (93.184.215.14): 2 of 3 ports open, checked from MY-PC.
- 22 SSH: no answer (filtered by a firewall, or the host is off)
- 80 HTTP: open, 12.6 ms
- 443 HTTPS: open, 12.9 ms
```

Structured data: `host`, `ip`, `from`, `timeout_ms`, `open`, `total`, and
`ports`, each with `port`, `service`, `state`, `status`, `ms`.

| `state` | What happened | What it usually means |
|---|---|---|
| `open` | The connection opened | A service is listening |
| `closed` | The host answered and refused | The host is up; nothing listens on that port |
| `no_answer` | Nothing came back in time | A firewall dropped it, or the host is off |
| `failed` | The attempt could not be made | See `status`, for example "Network unreachable" |

One TCP connection is opened to each port and closed the moment it opens. No
data is sent, so the check says a port is listening, not that the service
behind it is healthy. `service` is only the usual name for that port number.
The default wait is 3 seconds because Windows takes about two seconds to
report a refused connection; with a shorter wait a closed port can read as
`no_answer`.

#### trace_route

| Argument | Type | Meaning |
|---|---|---|
| `host` (required) | text | One host name or address |
| `max_hops` | 1 to 30, default 30 | How far to look |
| `resolve_names` | true or false, default true | Look up a name for each router |

Text returned:

```
Route to example.com (93.184.215.14): reached in 9 hops, traced from MY-PC.
 1  192.168.1.1  router.lan  0.52 ms
 2  *  no answer
 3  203.0.113.9  -  8.1 ms
 ...
```

Structured data: `host`, `ip`, `from`, `reached`, and `hops`, each with
`hop`, `address`, `name`, `ms`, `is_destination`.

Silent hops are normal: many routers do not answer. What matters is whether
the destination was reached and, if not, the last router that did answer.

#### network_info

No arguments. Returns this computer's name, the address it sends from by
default, and every interface address. Nothing is sent. Call it first when the
question depends on which network the checks start from.

Structured data: `computer`, `default_source_ipv4`, `default_source_ipv6`,
and `interfaces`, each with `interface`, `address`, `loopback`.

#### table_status

Reads the table of the pingtable app running on this computer.

| Argument | Type | Meaning |
|---|---|---|
| `only` | `all`, `down` or `up`, default `all` | Which hosts to list. `down` includes hosts that are failing but not yet counted as down |

Text returned:

```
pingtable on MY-PC is pinging: 12 hosts, 10 up, 2 down, 0 muted.
- printer.lan (192.168.1.31) icmp "Office printer": down, Timed out, 48 failed in a row
- nas.lan (192.168.1.20) tcp:445 "NAS file sharing": down, Connection refused, 12 failed in a row
```

Structured data: `computer`, `running`, `counts`, `listed`, and `hosts`, each
with `host`, `ip`, `check`, `description`, `group`, `state`, `status`,
`failed_in_a_row`, `sent`, `loss_pct`, `last_ms`, `avg_ms`.

`state` is `up`, `down`, `failing` (failed, but not yet often enough to count
as down), `recovering`, `pending` (no result yet) or `muted`.

If the app is not running the tool says so. Use `ping` for a one-off check
then, or ask the user to start pingtable.

#### table_add_hosts

Adds hosts to the running app's table. **This changes the user's saved host
list**, so ask before using it unless they asked for exactly that.

| Argument | Type | Meaning |
|---|---|---|
| `hosts` (required) | list of text | One host per entry, optionally followed by a description: `nas.lan NAS in the closet` |
| `group` | text | Group to put them in; created if it does not exist |
| `ports` | list of whole numbers, at most 64 | TCP ports to watch on each host: one row for each port |
| `ping` | true or false | With `ports`: also add the plain ping row. Defaults to false when ports are given |

Text returned:

```
Added 2 rows to the table, in group Home lab.
```

If pinging was stopped it is started, as the app's own Add window does.

### Which tool when

- "Is X up?" Use `ping`. If there is no answer, use `check_ports` on a port X
  should serve before saying it is down.
- "Is the web site, SSH, the database reachable?" Use `check_ports` with that
  port.
- "X does not answer; where is the problem?" Use `trace_route`, and
  `network_info` to see where the checks start from.
- "What is down right now?" Use `table_status` with `only` set to `down`.
- "Keep an eye on X." Use `table_add_hosts`, after asking.

### Limits and good manners

- One `ping` call: at most 256 hosts and 10 pings each.
- One `check_ports` call: one host and at most 64 ports. It checks the ports
  that are named; it is not a scanner. Prefer a few ports to a range.
- Check only hosts the user owns or is allowed to test. Port checks against
  someone else's machines can look like an attack.
- Nothing here needs administrator rights, and nothing is kept between
  calls except what `table_add_hosts` adds.

### Protocol notes

The server speaks protocol revisions 2024-11-05 through 2025-11-25, which
open with an `initialize` handshake, and 2026-07-28, which has none
(`server/discover` lists the supported versions). It offers tools only: no
resources or prompts. Messages are one JSON-RPC object per line. Diagnostics
go to standard error and to `pingtable.log`, never to standard output.

Try it by hand:

```
echo {"jsonrpc":"2.0","id":1,"method":"tools/list"} | pingtable-cli mcp
```

## Command line

An agent that can run commands does not need MCP.

```
pingtable-cli once --format json 192.168.1.1 example.com:443
pingtable-cli ports --format json example.com 22 443
```

Both print one JSON document and exit with status 0 when every host answered
or every port was open, 1 when not, and 2 when the command could not be
understood. Errors go to standard error.

`ports` prints:

```json
{
  "schema_version": 1,
  "host": "example.com",
  "target": "93.184.215.14",
  "timeout_ms": 3000,
  "open": 1,
  "results": [
    { "port": 22, "service": "SSH", "state": "no_answer", "status": "timeout", "rtt_us": null },
    { "port": 443, "service": "HTTPS", "state": "open", "status": "ok", "rtt_us": 12900 }
  ]
}
```

`once` prints `schema_version`, `exported_at` and `hosts`, with the same
fields per host as [the table in the API](api.md#the-table). The ones that
matter most: `host`, `ip`, `probe`, `state`, `last_status`, `last_rtt_us`.

Times in these two outputs are microseconds (`rtt_us`); divide by 1000 for
milliseconds. The MCP tools already give milliseconds.

## HTTP API

Everything the app's window does goes through a local HTTP API, so an agent
or a script can do it too. It answers only on this computer and needs a
token.

```
pingtable-cli api
```

prints the address, the token and ready-made example calls. With
`--format json` it prints them for a program:

```json
{
  "schema_version": 1,
  "base_url": "http://127.0.0.1:7464/api/v1",
  "header": "X-Pingtable-Token",
  "token": "…",
  "running": true,
  "config_path": "C:\\Users\\you\\AppData\\Roaming\\pingtable\\pingtable.json"
}
```

The API exists only while pingtable is running (`"running": true`): start
`pingtable.exe`, or run `pingtable-cli` with no arguments.

The token is a password for this computer's pingtable. The settings it
guards include commands pingtable runs on alerts, so do not paste the token
into a prompt for a hosted model and do not write it to shared files. An
agent that needs the table should use the MCP tools, which read the token
themselves and never show it.

Every call, with examples, is in [api.md](api.md).

## When something does not work

- **The client shows no tools.** The path to `pingtable-cli.exe` is wrong, or
  the client was not restarted after the config change. Run the "try it by
  hand" line above; it should print one line of JSON.
- **`table_status` says the app is not running.** Start `pingtable.exe`. The
  other four tools work without it.
- **Everything times out.** Security software or a firewall may be blocking
  outgoing pings from `pingtable-cli.exe`. Try `check_ports` on a known web
  site; a TCP check uses ordinary outgoing connections.
- **Inside WSL or a container,** the checks start from that environment's
  network, not from Windows. `network_info` shows which one.
