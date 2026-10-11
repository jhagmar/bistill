# Linux

Install `curl` and make sure it is on `PATH`. bistill uses it for every Bitbucket request and checks TLS with the operating system's trust store.

Follow the token and config steps in the [README](../README.md). The token file must be mode `600`.

## Tray

Leave `bistill` open in a terminal. That process registers one status icon on the session bus. The tooltip is the unread count. Activating the icon raises this terminal.

A desktop that is already watching status icons shows the icon. If no watcher is running, bistill writes that to the log once and keeps polling.
