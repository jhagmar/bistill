# bistill

The Bitbucket list you were missing.

[![CI](https://github.com/jhagmar/bistill/actions/workflows/ci.yml/badge.svg)](https://github.com/jhagmar/bistill/actions/workflows/ci.yml)
[![CodeQL](https://github.com/jhagmar/bistill/actions/workflows/codeql.yml/badge.svg)](https://github.com/jhagmar/bistill/actions/workflows/codeql.yml)
[![codecov](https://codecov.io/gh/jhagmar/bistill/graph/badge.svg)](https://codecov.io/gh/jhagmar/bistill)
[![OpenSSF Scorecard](https://api.scorecard.dev/projects/github.com/jhagmar/bistill/badge)](https://scorecard.dev/viewer/?uri=github.com/jhagmar/bistill)
[![REUSE status](https://api.reuse.software/badge/github.com/jhagmar/bistill)](https://api.reuse.software/info/github.com/jhagmar/bistill)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Contributor Covenant](https://img.shields.io/badge/Contributor%20Covenant-2.1-4baaaa.svg)](CODE_OF_CONDUCT.md)

bistill lists the Bitbucket pull requests that need your review, and the open pull requests you created. It runs on your computer and uses your HTTP access token. You do the actual review in Bitbucket. bistill only sends read requests.

Open a terminal and leave `bistill` running. That process polls, draws the inbox, and shows an unread count on a tray icon.

## What you need

`curl` on Linux, or `curl.exe` on Windows, must be on `PATH`. bistill uses it for every request and checks TLS with the operating system's trust store. Each request waits up to 15 seconds.

You also need an HTTP access token that can read pull requests on that server. bistill keeps the token in memory and does not print it. If you pass `--verbose`, the log shows `Authorization: Bearer ***`.

## Set it up

bistill looks for configuration in this order: `--config PATH`, then `./bistill.conf` in the current directory, then the user config file. On Linux that file is `$XDG_CONFIG_HOME/bistill/config`, which is `~/.config/bistill/config` when `XDG_CONFIG_HOME` is unset. On Windows it is `%APPDATA%\bistill\config`.

The file is UTF-8 text, one `key = value` per line. A `#` starts a comment. Write paths out in full. bistill does not expand `~`, so `~/...` is not your home directory.

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

On Windows, limit the token file to your user account. bistill does not read the ACL, so set that permission yourself.

If the same setting appears in more than one place, the flag wins, then the environment variable, then the file. The variables are `BISTILL_URL`, `BISTILL_USER`, `BISTILL_TOKEN`, and `BISTILL_TOKEN_FILE`. If `BISTILL_TOKEN` is set, bistill uses that value.

Check the connection before you rely on the list:

```
bistill ping --url https://git.example.invalid --user jcitizen
```

A successful `ping` prints the curl version, confirms TLS, and then prints the Bitbucket name and version, your display name, and how many pull requests are in the inbox. `--json` prints the raw response bodies.

## Day to day

Open a terminal and run `bistill`, or `bistill tui`. That is the inbox. A second `bistill` exits 1 and names the process that already holds the lock. The screen has two sections, **Needs review** and **Waiting**. Needs review holds open pull requests where you are a reviewer, including ones you have already approved. Waiting holds open pull requests you wrote. If you are both the author and a reviewer, the row stays under Needs review.

On a wide terminal the table and the detail sit side by side. On a narrow one the table is above the detail. The bottom line is the status and the keys that apply right now. An empty inbox says "Nothing needs your attention."

The detail lists activity newest first: the time, the person, a short verb, and the comment or commit subject. A line you have not caught up to starts with `new`. Selecting a row, or pressing `m`, marks that pull request read. Press `i` on a Needs review row to ignore it. An ignored row shows the badge `ignored` and stays out of the tray count. Opening the pull request in the browser does not mark it read.

The tray icon's tooltip is the number of unread pull requests. Activating the icon raises this terminal. On Linux the icon is a status notifier item. On Windows it is a notification-area icon. If no status-icon watcher is running, bistill writes that to the log once and keeps polling.

The process writes `snapshot.json` as each reply arrives, and `watermarks.json` when you catch up or ignore a row. Both files live in the state directory, `~/.local/state/bistill` on Linux (or `$XDG_STATE_HOME/bistill`) and `%LOCALAPPDATA%\bistill` on Windows. The first successful poll marks the current inbox read, so pull requests that were already there do not light the tray. Later events do.

Press `q` to leave. The terminal returns to the screen you had before, and the tray icon goes away.

| Key | What it does |
| --- | --- |
| `j` / `k` or arrows | Move the selection and mark that pull request read |
| `m` | Mark the selected pull request read |
| `i` | Ignore a Needs review row, or count it in the tray again |
| Enter | Open the pull request |
| `r` | Ask for a fetch. It starts once the poll interval has passed since the last one |
| `/` | Filter by title, repository, or author. The match ignores case. Escape cancels. Enter applies. Enter on an empty line clears the filter |
| Tab | Switch section |
| `?` | Show these keys |
| `q` | Quit and restore the terminal |

A click selects the row under the pointer and marks it read. The wheel scrolls the pane that has focus. A second click on the same row within 400 milliseconds opens the pull request.

Linux tray notes are in [docs/linux.md](docs/linux.md). Windows notes are in [docs/windows.md](docs/windows.md).

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
| `log_file` | Where warnings and errors are written. Unset means stderr for `ping`. The inbox keeps those lines off the screen and shows status in the footer |
| `state_dir` | Where `snapshot.json`, `watermarks.json`, and the lock live. Default as above |

`ping` and `tui` accept `--url`, `--user`, `--token-file`, `--config`, and `--verbose`. `ping` also accepts `--json`. `bistill --help` lists them.

## When a command fails

| Exit | Cause |
| --- | --- |
| 0 | Success, including an empty inbox |
| 1 | Usage, configuration, or another bistill already holds the lock |
| 2 | `curl` or `curl.exe` is missing from `PATH` |
| 3 | TLS verification failed |
| 4 | The request timed out |
| 5 | The response was not valid JSON |
| 11 | HTTP 401. The message is "Token rejected." |
| 12 | HTTP 403 |
| 13 | HTTP 404 |
| 10 | Any other HTTP status |

Running `bistill` with output that is not a terminal prints "Run ping, or start the inbox with bistill tui." and exits 1. `bistill tui` with output that is not a terminal prints "Open a terminal to start the inbox." and exits 1.

## Build

The minimum supported Rust is 1.85. The edition is 2024. GitHub runs the checks on Rust 1.85.0.

```
cargo test --workspace --locked
```

The release binary is `target/release/bistill` after `cargo build --release -p bistill`. Put that directory on `PATH`.

To run the same checks as GitHub on your machine, see [ci/README.md](ci/README.md). Contributor setup is in [CONTRIBUTING.md](CONTRIBUTING.md).

## Security

Report a vulnerability in private. See [SECURITY.md](SECURITY.md).

## License

MIT. Copyright (c) 2026 Jonas Hagmar.
