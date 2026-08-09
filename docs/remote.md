# Running the daemon on another machine

The daemon binds loopback only. It spawns the processes you ask it to, so a
listener on the network is a remote shell for whoever finds it. Reach a remote
daemon over SSH.

## On the remote machine

Install as usual, then keep the daemon alive across logouts:

```sh
mkdir -p ~/.config/systemd/user
cp packaging/vitrum-server.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now vitrum-server
loginctl enable-linger "$USER"
```

`loginctl enable-linger` is the line that matters. Without it, systemd tears
down your user's services when your last session ends, so closing the SSH
connection kills every agent. With it, the daemon belongs to the machine.

## On your machine

```sh
ssh -N -L 7737:127.0.0.1:7737 user@host &
vitrum --server ws://127.0.0.1:7737
```

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
daemon requires reparenting them, which is not built. `DESIGN-REMOTE.md` has
the design.
