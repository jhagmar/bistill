# bistill

The Bitbucket list you were missing.

A personal list of Bitbucket pull requests that need your review, and open
pull requests you authored. An OS notification when that list changes. The
review is written on Bitbucket. It runs on the laptop that can already reach
Bitbucket, as you, with your token.

## ping

`bistill ping` checks that curl can reach Bitbucket and prints the server
version, your display name, and the inbox count. `--json` prints the raw
bodies. curl must be on PATH. The token is `BISTILL_TOKEN` or `token_file`.

```
bistill ping --url https://git.example.invalid --user jcitizen
```

## ls

`bistill ls` prints Needs review and Waiting on others. An empty list prints
"Nothing needs your attention." and exits 0. `--count` prints how many pull
requests need you. `--json` prints the snapshot. A successful `ls` writes
`snapshot.json` under `state_dir`. When a pull request in that snapshot
changes, `ls` sends one notification for it. The title is `Bistill`. The
body names the pull request, the reasons, and the link. If `notify-send` is
missing, `ls` logs that once and still prints the list.

```
bistill ls --url https://git.example.invalid --user jcitizen
```

```
notify-send --expire-time 10000 -- Bistill "PRJ/repo#12 needs review
https://git.example.invalid/projects/PRJ/repos/repo/pull-requests/12"
```

On Windows, `ls` shows a PowerShell toast. A click opens the pull request.

## Build

MSRV is 1.85. Edition 2024.

```
cargo test --workspace --locked
```
