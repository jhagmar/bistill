# bistill

The Bitbucket list you were missing.

[![CI](https://github.com/jhagmar/bistill/actions/workflows/ci.yml/badge.svg)](https://github.com/jhagmar/bistill/actions/workflows/ci.yml)
[![CodeQL](https://github.com/jhagmar/bistill/actions/workflows/codeql.yml/badge.svg)](https://github.com/jhagmar/bistill/actions/workflows/codeql.yml)
[![codecov](https://codecov.io/gh/jhagmar/bistill/graph/badge.svg)](https://codecov.io/gh/jhagmar/bistill)
[![OpenSSF Scorecard](https://api.scorecard.dev/projects/github.com/jhagmar/bistill/badge)](https://scorecard.dev/viewer/?uri=github.com/jhagmar/bistill)
[![REUSE status](https://api.reuse.software/badge/github.com/jhagmar/bistill)](https://api.reuse.software/info/github.com/jhagmar/bistill)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Contributor Covenant](https://img.shields.io/badge/Contributor%20Covenant-2.1-4baaaa.svg)](CODE_OF_CONDUCT.md)

Bitbucket already knows which pull requests need your review, and which open pull requests you wrote. That inbox is easy to leave closed. bistill keeps the same list on the laptop that can already reach your team's Bitbucket, signed in as you, and sends a notification when the list changes. You write the review on Bitbucket. Every request bistill makes is a read.

One process polls. A terminal shows the list. A notification names the pull request, why it changed, and the link.

## What you need

`curl` on Linux, or `curl.exe` on Windows, must be on `PATH`. bistill uses it for every request and checks TLS with the operating system's trust store. Each request waits up to 15 seconds.

You also need an HTTP access token that can read pull requests on that server. The token stays in this process. It is never printed. With `--verbose`, the log shows `Authorization: Bearer ***`.

## Set it up

bistill looks for configuration in this order: `--config PATH`, then `./bistill.conf` in the current directory, then the user config file. On Linux that file is `$XDG_CONFIG_HOME/bistill/config`, which is `~/.config/bistill/config` when `XDG_CONFIG_HOME` is unset. On Windows it is `%APPDATA%\bistill\config`.

The file is UTF-8 text, one `key = value` per line. A `#` starts a comment. Write paths in full. A leading `~` stays as those two characters.

```
base_url = https://git.example.invalid
username = jcitizen
token_file = /home/jcitizen/.config/bistill/token
```

`base_url` is the Bitbucket origin for your team, with no trailing slash. Include a context path when the server has one. `username` is your Bitbucket user slug.

Put the token alone in the file named by `token_file`. On Linux, that file must be readable and writable by you alone. bistill refuses a token file the group or anyone else can read:

```
mkdir -p ~/.config/bistill
umask 077
printf '%s\n' 'paste-the-token-here' > ~/.config/bistill/token
```

On Windows, give that file access only to your user account. bistill does not read the ACL. It still expects the file to be private.

A flag overrides an environment variable, and an environment variable overrides the file. `BISTILL_URL`, `BISTILL_USER`, `BISTILL_TOKEN`, and `BISTILL_TOKEN_FILE` are the variables. When `BISTILL_TOKEN` is set, bistill uses it and leaves the token file unread.

Check the connection before you rely on the list:

```
bistill ping --url https://git.example.invalid --user jcitizen
```

A successful `ping` prints the curl version, confirms TLS, and then prints the Bitbucket name and version, your display name, and how many pull requests are in the inbox. `--json` prints the raw response bodies.

## Day to day

Open a terminal and run `bistill` with no subcommand. That is the inbox. The screen has two sections, **Needs review** and **Waiting**. Needs review holds pull requests where you are a reviewer and still have something to do. Waiting holds open pull requests you authored, and reviews where you are already done and the next step belongs to someone else.

On a wide terminal the table and the detail sit side by side. On a narrow one the table is above the detail. The bottom line is the status. An empty inbox says "Nothing needs your attention."

The first 50 pull requests, oldest update first, include unanswered threads, open tasks, the build, and whether the pull request can merge. Further rows stay in the list. When there are more than 50, the detail starts with "and N more".

Press `q` to leave. The terminal returns to the screen you had before.

| Key | What it does |
| --- | --- |
| `j` / `k` or arrows | Move the selection |
| Enter | Open the pull request |
| `r` | Ask for a fetch. It starts once the poll interval has passed since the last one |
| `/` | Filter by title, repository, or author. The match ignores case. Escape cancels. Enter applies. Enter on an empty line clears the filter |
| Tab | Switch section |
| `?` | Show these keys |
| `q` | Quit and restore the terminal |

A click selects the row under the pointer. The wheel scrolls the pane that has focus. A second click on the same row within 400 milliseconds opens the pull request.

`bistill ls` prints the same two sections and exits. An empty list prints "Nothing needs your attention." and exits 0. `--count` prints how many pull requests need you: everything under Needs review, plus Waiting rows that have an unanswered author thread or an open task. `--json` prints the snapshot.

```
bistill ls --url https://git.example.invalid --user jcitizen
```

`ls` and the background poller both write `snapshot.json` as each reply arrives. The file lives in the state directory, `~/.local/state/bistill` on Linux (or `$XDG_STATE_HOME/bistill`) and `%LOCALAPPDATA%\bistill` on Windows. After a restart, bistill compares the new list with that file, so a pull request you already knew about stays quiet.

When a pull request in the snapshot changes, you get one notification. The title is `Bistill`. The body names the pull request, the reason, and the link. A pull request that left the inbox is called merged or declined when bistill can read that pull request's state. When that read fails, the notification still says merged or declined.

```
notify-send --expire-time 10000 -- Bistill "PRJ/repo#12 needs review
https://git.example.invalid/projects/PRJ/repos/repo/pull-requests/12"
```

On Windows the notification is a PowerShell toast. A click opens the pull request.

`bistill watch` is the same poller without a screen. Leave it running and you get the notifications while you work. A second `bistill watch` exits 1 and names the process that already holds the lock. If that process is gone, the lock file is free and the next `watch` takes it.

Run `bistill` in a terminal while `watch` is already running and this terminal only displays the list. It re-reads `snapshot.json` about once a second. Polling and notifications stay with the first process. `r` in that window asks the poller to fetch again.

Linux setup for a user service is in [docs/linux.md](docs/linux.md). Windows setup for a Startup shortcut is in [docs/windows.md](docs/windows.md).

While a fetch is in progress the status line says "Fetching from Bitbucket..." and the rows you already have stay on screen. A rejected token says "Token rejected." A TLS failure says "curl failed TLS." A server you cannot reach says "Bitbucket unreachable (since …)." Too many requests says "Rate limited." The poller keeps the last rows and tries again. The wait starts at your poll interval, at least 15 seconds, doubles after each failure, and stops at 10 minutes. A rate-limit response can name its own wait.

## Configuration

| Key | Meaning |
| --- | --- |
| `base_url` | Bitbucket origin. Required. No trailing slash |
| `username` | Your user slug. Required |
| `token_file` | File that contains the token. Required unless `BISTILL_TOKEN` is set |
| `poll_seconds` | How often to poll. Default 60. A value below 15 is treated as 15 |
| `stale_days` | A Waiting row older than this shows the badge `stale`. Default 7 |
| `ca_file` | PEM file for a private certificate authority. Unset means the operating system trust store |
| `log_file` | Where warnings and errors are written. Unset means stderr for `ping`, `ls`, and `watch`. The inbox keeps those lines off the screen and shows status in the footer |
| `state_dir` | Where `snapshot.json` and the lock live. Default as above |

`ping`, `ls`, and `watch` accept `--config` and `--verbose`. `ping` also accepts `--url`, `--user`, `--token-file`, and `--json`. `ls` also accepts `--json` and `--count`. `bistill --help` lists them.

## When a command fails

| Exit | Cause |
| --- | --- |
| 0 | Success, including an empty inbox |
| 1 | Usage, configuration, or a `watch` lock already held |
| 2 | `curl` or `curl.exe` is missing from `PATH` |
| 3 | TLS verification failed |
| 4 | The request timed out |
| 5 | The response was not valid JSON |
| 11 | HTTP 401. The message is "Token rejected." |
| 12 | HTTP 403 |
| 13 | HTTP 404 |
| 10 | Any other HTTP status |

Running `bistill` with output that is not a terminal prints "Run ping, ls, or watch." and exits 1.

## Build

The minimum supported Rust is 1.85. The edition is 2024. GitHub runs the checks on Rust 1.85.0.

```
cargo test --workspace --locked
```

The release binary is `target/release/bistill` after `cargo build --release -p bistill`. Put that directory on `PATH`, or use the full path in the service and the shortcut.

To run the same checks as GitHub on your machine, see [ci/README.md](ci/README.md). Contributor setup is in [CONTRIBUTING.md](CONTRIBUTING.md).

## Security

Report a vulnerability in private. See [SECURITY.md](SECURITY.md).

## License

MIT. Copyright (c) 2026 Jonas Hagmar.
