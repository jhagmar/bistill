# Windows

Install `curl.exe` and make sure it is on `PATH`. bistill uses it for every Bitbucket request and checks TLS with the operating system's trust store.

Use Windows Terminal or the Windows 10/11 console. `q` in the inbox restores the screen you had before.

Follow the token and config steps in the [README](../README.md). Limit the token file to your user account. bistill does not read the ACL, so set that permission yourself.

## Tray

Leave `bistill` open in a terminal. That process adds one icon to the notification area. The tooltip is the unread count. Activating the icon raises this terminal.
