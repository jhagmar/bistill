# Windows

`curl.exe` must be on `PATH`. bistill uses it for every Bitbucket request and checks TLS with the OS trust store.

Notifications use a PowerShell toast. If the toast fails, bistill logs that once and keeps polling. A click opens the pull request when Windows allows it.

A Startup shortcut runs `bistill watch`. The binary does not create the shortcut.

Use Windows Terminal or the Windows 10/11 console. `q` in the inbox restores the screen.
