# Linux

Install `curl` and make sure it is on `PATH`. bistill uses it for every Bitbucket request and checks TLS with the operating system's trust store.

Notifications use `notify-send`. Install the package that provides that command. If it is missing, bistill writes that to the log once and keeps polling. The notification body contains the pull request link.

Follow the token and config steps in the [README](../README.md). The token file must be mode `600`.

## Keep the poller running

`bistill watch` polls and sends notifications. bistill will not install a service for you. Save this as a user service:

```
# ~/.config/systemd/user/bistill.service
[Unit]
Description=Bitbucket inbox poller

[Service]
ExecStart=bistill watch
Restart=on-failure

[Install]
WantedBy=default.target
```

`bistill` in `ExecStart` must be on the `PATH` systemd gives the service, or replace it with the full path to the binary.

```
systemctl --user daemon-reload
systemctl --user enable --now bistill.service
```

A user service stops when your login session ends. This keeps it running after you log out:

```
loginctl enable-linger "$USER"
```

Logs from the service go to the journal. `journalctl --user -u bistill.service` shows them. Set `log_file` in the config when you also want a file.

Open `bistill` in a terminal while the service is running and the terminal shows the list. Polling stays with the service. `q` restores the terminal and leaves the service running.
