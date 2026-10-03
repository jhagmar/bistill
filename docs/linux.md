# Linux

`curl` must be on `PATH`. bistill uses it for every Bitbucket request and checks TLS with the OS trust store.

Notifications use `notify-send`. If it is missing, bistill logs that once and keeps polling.

A systemd user service runs the poller. The binary does not install the unit.

```
[Service]
ExecStart=bistill watch
```

`q` in the inbox restores the terminal.
