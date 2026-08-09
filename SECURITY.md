# Security Policy

## Supported versions

vitrum is at 0.1.0. Fixes go to the 0.1.x line, released as a new tag with a
prebuilt archive and a `SHA256SUMS` file beside it. There is no long-term
support branch and no backporting to earlier tags.

| Version | Supported |
|---|---|
| 0.1.x | yes |
| anything older | no |

`vitrum update` installs the latest release, and it refuses a release that
publishes no sums. Updating is the way a fix reaches you.

## Reporting a vulnerability

Do not open a public issue, a pull request, or a discussion for a security
problem.

Report it privately through GitHub private vulnerability reporting on this
repository:

    https://github.com/santhreal/vitrum/security/advisories/new

If that form is unavailable to you, contact the maintainer through
https://github.com/santhreal.

A useful report has the version (`vitrum --version`), the platform, what an
attacker starts with, what they end up with, and the shortest sequence that
gets from one to the other. A patch is welcome but not required.

## What to expect

The project is maintained by one person, so an answer is not instant. You can
expect an acknowledgement that the report was read, a judgement on whether it
is in scope with the reasoning either way, and if it is in scope, a fix in a
0.1.x release with the issue named in `CHANGELOG.md`. A credit in the advisory
if you want one. There is no bounty.

Please give the fix a chance to ship before disclosing publicly. A report ruled
out of scope carries no such request; publish it whenever you like.

## Scope

vitrum is a terminal. It exists to run the programs you name, in a real PTY,
with your privileges. **That a session executes what the operator told it to
execute is the product, not a vulnerability.** A report that amounts to
"vitrum ran my command", "a shell in vitrum can read my files", or "an agent
started from the launcher deleted something" is not a security issue here.

What is a security issue is any way for a **remote party, another local user,
or a less privileged process** to reach that execution, or to read state it
should not have. The trust boundaries worth attacking:

**The daemon's local socket.** `vitrum-server` listens on a TCP port bound to
`127.0.0.1` (`Ipv4Addr::LOCALHOST`, default port 7737) and speaks WebSocket.
The client connects to it and sends `ClientMsg::Hello { protocol }`; the
daemon answers `ServerMsg::Welcome` only if the number matches its own
`PROTOCOL_VERSION`, and refuses anything sent before the hello. See
`crates/vitrum-proto/src/lib.rs` for the message set and
`crates/vitrum-server/src/conn.rs` for the exchange. Past the handshake, a
connected client can list projects, create sessions, and therefore run
programs. So: anything that gets that port reachable off the loopback
interface, any way to get the daemon to act on an unauthenticated or
pre-handshake message, a parser bug in a control or data frame that a
malicious peer can drive, or a cross-site request from a browser that reaches
the WebSocket, is in scope.

**Bytes coming out of a PTY.** A session's output is written by whatever the
operator ran, and that program's output may itself come from a network. An
escape sequence, a control frame, or a scrollback record that makes the client
read a file, run something, or corrupt memory rather than just draw is in
scope.

**The `vitrum-backdrop://` scheme.** The client registers a custom scheme
(`app/src/chrome.rs`) so the page can display a backdrop image, and it serves
the path named in the operator's own profile, from the operator's own
filesystem. It is deliberately not confined to a directory, because a
wallpaper lives wherever the operator keeps wallpapers, and it refuses any
file whose bytes are not an image so a path cannot be turned into a general
file-read primitive. A way to make it return non-image bytes, to reach a path
the operator never chose, or to have anything other than the operator choose
the path, is in scope.

**Program launch.** `app/src/launch.rs` resolves the agent commands the
new-session dialog offers and starts `vitrum-server` when nothing is
listening. Argument or path handling that lets untrusted input decide which
binary runs, or that picks up a binary from a directory an attacker can write,
is in scope.

**The updater.** `vitrum update` downloads a release archive named
`vitrum-<version>-<target>.tar.gz` and verifies it against the release's
`SHA256SUMS`. Anything that gets unverified bytes onto disk or executed, or
that writes outside the install directory while unpacking, is in scope.

Out of scope, besides operator-directed execution: findings against
`vendor/` or `vendor-pty/` that are upstream's and reproduce against
upstream `dioxus-desktop` or `portable-pty` (report those upstream, and tell
us so the fork can absorb the fix), issues in the system WebKit runtime,
missing hardening that has no demonstrated impact, and reports produced by a
scanner with no working path to abuse.
