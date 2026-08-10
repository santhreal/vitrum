# Running the daemon on another machine

The daemon listens on `127.0.0.1` and never on `0.0.0.0`. It is a single-user
service: it spawns the processes you ask it to, with your privileges. Reaching
it from another machine means an SSH tunnel to loopback on that machine, plus a
copy of that machine's token.

## Authentication

The daemon generates 32 random bytes at startup and writes them hex-encoded to
`$XDG_RUNTIME_DIR/vitrum/token` when that variable is set, and to the data
directory otherwise. The file is mode 0600 inside a 0700 directory. A new token
is written on every start.

The client takes the token from `VITRUM_TOKEN`, then from `--token-file`, then
from the default local path. The secret is never passed in argv, which other
users on the machine can read.

A WebSocket handshake that carries an `Origin` header is refused with 403. A
native client sends none and a browser always does, so a web page cannot drive
the daemon.

The daemon serves 64 connections at once. A client past that waits for a slot
rather than being refused, and gets one when another connection closes.

`PROTOCOL_VERSION` is 3. A client and a daemon that disagree refuse each other
and name both versions, which is what a mixed-version pair across two machines
looks like.

## On the remote machine

Install as usual, then keep the daemon alive across logouts:

```sh
mkdir -p ~/.config/systemd/user
cp packaging/vitrum-server.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now vitrum-server
loginctl enable-linger "$USER"
```

Without `loginctl enable-linger`, systemd tears down your user's services when
your last session ends, so closing the SSH connection kills every agent. With
it, the daemon belongs to the machine.

## On your machine

Copy the remote token, open the tunnel, and point the client at it:

```sh
ssh user@host 'cat "$XDG_RUNTIME_DIR/vitrum/token"' > ~/.config/vitrum/remote-token
chmod 600 ~/.config/vitrum/remote-token
ssh -N -L 7737:127.0.0.1:7737 user@host &
vitrum --server ws://127.0.0.1:7737 --token-file ~/.config/vitrum/remote-token
```

Copy the token again after the remote daemon restarts.

The window reconnects on its own after a network loss, backing off from a
quarter second to a thirty second ceiling, and resumes each session at the byte
it stopped at.

The schedule is finite: 25 attempts, about ten minutes once it reaches the
ceiling. After that the window reports the failure and waits for Retry. The
agents are unaffected either way; they belong to the daemon.

## What survives what

| Event | Sessions |
|---|---|
| Close a window | keep running |
| Close every window | keep running |
| Lose the network or the client | keep running; the window reconnects and resumes |
| Log out of the remote host | keep running, with `enable-linger` set |
| The daemon crashes or is upgraded | every session dies |

The PTYs are the daemon's children, and no flag changes that. Outliving the
daemon requires reparenting them, which is not built.
