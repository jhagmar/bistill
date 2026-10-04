# bistill

The Bitbucket list you were missing.

A personal list of Bitbucket pull requests that need your review, and open
pull requests you authored. An OS notification when that list changes. The
review is written on Bitbucket. It runs on the laptop that can already reach
Bitbucket, as you, with your token.

[![CI](https://github.com/jhagmar/bistill/actions/workflows/ci.yml/badge.svg)](https://github.com/jhagmar/bistill/actions/workflows/ci.yml)
[![CodeQL](https://github.com/jhagmar/bistill/actions/workflows/codeql.yml/badge.svg)](https://github.com/jhagmar/bistill/actions/workflows/codeql.yml)
[![codecov](https://codecov.io/gh/jhagmar/bistill/graph/badge.svg)](https://codecov.io/gh/jhagmar/bistill)
[![OpenSSF Scorecard](https://api.scorecard.dev/projects/github.com/jhagmar/bistill/badge)](https://scorecard.dev/viewer/?uri=github.com/jhagmar/bistill)
[![REUSE status](https://api.reuse.software/badge/github.com/jhagmar/bistill)](https://api.reuse.software/info/github.com/jhagmar/bistill)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Contributor Covenant](https://img.shields.io/badge/Contributor%20Covenant-2.1-4baaaa.svg)](CODE_OF_CONDUCT.md)

## ping

`bistill ping` checks that curl can reach Bitbucket and prints the server
version, your display name, and the inbox count. `--json` prints the raw
bodies. curl must be on PATH. The token is `BISTILL_TOKEN` or `token_file`.

```
bistill ping --url https://git.example.invalid --user jcitizen
```

## ls

`bistill ls` prints Needs review and Waiting on others. An empty list prints
"Nothing needs your attention." and exits 0. The first 50 pull requests
include unanswered threads, open tasks, the build, and merge. `--count`
prints how many pull requests need you. Waiting rows count when they have
unanswered author threads or open tasks. `--json` prints the snapshot. A
successful `ls` writes `snapshot.json` under `state_dir` as each reply is
applied. When a pull request in that snapshot changes, `ls` sends one
notification for it. The title is `Bistill`. The body names the pull request,
the reasons, and the link. A pull request that left the inbox is named
`merged` or `declined` when that pull request's state says so. If the read
fails, the notification says merged or declined. If `notify-send` is missing, `ls` logs that once and still prints the list.

```
bistill ls --url https://git.example.invalid --user jcitizen
```

```
notify-send --expire-time 10000 -- Bistill "PRJ/repo#12 needs review
https://git.example.invalid/projects/PRJ/repos/repo/pull-requests/12"
```

On Windows, `ls` shows a PowerShell toast. A click opens the pull request.

## Inbox

With no subcommand, a terminal on stdout opens the inbox. The process that
takes `poll.lock` polls on a thread and sends the same notifications as
`watch`. A second `bistill` in a terminal reads `snapshot.json` and leaves
polling to that process. `q` restores the terminal. Any other stdout prints
usage and exits 1.

## watch

`bistill watch` polls on this process and sends a notification when the inbox
changes. It writes `snapshot.json`. A second `bistill watch` exits 1 and names
the pid that holds the lock. A pid that is not running leaves the lock free.

Linux setup, including a systemd user service, is in `docs/linux.md`. Windows
setup, including a Startup shortcut, is in `docs/windows.md`.

## Build

MSRV is 1.85. Edition 2024. CI runs on Rust 1.85.0.

```
cargo test --workspace --locked
```

The same checks as GitHub, including REUSE, run in Docker. See [ci/README.md](ci/README.md). Host commands are in [CONTRIBUTING.md](CONTRIBUTING.md).

## Security

See [SECURITY.md](SECURITY.md).

## License

MIT. Copyright (c) 2026 Jonas Hagmar.
