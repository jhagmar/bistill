# Windows

Install `curl.exe` and make sure it is on `PATH`. bistill uses it for every Bitbucket request and checks TLS with the operating system's trust store.

Use Windows Terminal or the Windows 10/11 console. `q` in the inbox restores the screen you had before.

Notifications are a PowerShell toast. If the toast fails, bistill writes that to the log once and keeps polling. A click opens the pull request when Windows allows it.

Follow the token and config steps in the [README](../README.md). Give the token file access only to your user account. bistill does not read the ACL.

## Keep the poller running

`bistill watch` polls and sends notifications. The binary does not create a shortcut. A Startup shortcut does that.

1. Press Win+R, run `shell:startup`, and confirm the folder that opens.
2. Create a shortcut there. The target is the full path of `bistill.exe`, and the argument is `watch`. If `bistill.exe` is already on `PATH`, the target can be `bistill.exe` with the argument `watch`.
3. Name the shortcut bistill.

The shortcut runs when you sign in. Open `bistill` in a terminal while it is running and the terminal shows the list. Polling stays with the shortcut's process. `q` restores the screen and leaves that process running.
