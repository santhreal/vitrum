# vitrum-dioxus-desktop

A fork of [`dioxus-desktop`](https://crates.io/crates/dioxus-desktop) 0.7.10 by
Jonathan Kelley, carried by [vitrum](https://github.com/santhreal/vitrum) under
the MIT licence. Everything except the changes below is upstream's code.

## What diverges

Three files, listed with their reasons in `UPSTREAM.toml`, which is the
authoritative copy. `tools/upstream/check.sh` fails if the real divergence is
not exactly that list, so neither this section nor that file can drift from
what is actually in `src/`.

**`src/webview.rs`** is the reason the fork exists. On Linux each new webview is
built with `with_related_view`, pointed at a webview that is still alive, so
every window runs inside the first window's `WebKitWebProcess`.
`LIVE_WEBKIT_VIEWS` tracks which views are still real so the relation target is
never a dead widget.

**`src/app.rs`** drops one module-scope `Duration` import that is unused in this
crate's configuration, because a function below re-imports it. vitrum builds
with `-D warnings`, so an unused import is a build failure.

**`src/config.rs`** names this crate in one doc comment. Inert, since
`[lib] doctest = false`, but kept so the rendered docs are not wrong.

Upstream builds a fresh `WebContext` per webview, and on Linux each one starts
its own `WebKitNetworkProcess`. Measured on this project: 20 windows ran 20 of
them at 8.6 MB PSS each, so 171.2 MB went on twenty copies of one cache and
cookie jar. vitrum's memory target for 20 windows does not survive that.

## Why a fork and not a patch

`[patch.crates-io]` applies only to a workspace build. Anyone running
`cargo install vitrum` resolves the registry copy instead, so the published
client would have silently shipped the per-window process behaviour. A named
dependency is the only form of this change that survives publication.

## Staying current

A fork nobody diffs becomes a permanent one. `tools/upstream/check.sh` is the
thing that stops that happening. It downloads the pristine crate at the version
in `UPSTREAM.toml`, confirms the divergence is exactly what is declared, and
then asks crates.io whether anything newer is out.

```sh
sh tools/upstream/check.sh
```

It runs weekly in CI, so nobody has to remember. Three ways it fails, and each
means something different:

| It says | What happened |
|---|---|
| diverges but is not declared | someone edited `vendor/src` without recording why |
| declared but no longer differs | a divergence went dead and should be dropped |
| upstream is at a newer version | time to absorb |

To absorb a release:

1. Extract what this fork changed, as patches against the release it forked:

   ```sh
   sh tools/upstream/check.sh --patches /tmp/fork
   ```

2. Download and unpack the new crate somewhere outside the repository, and
   replace `vendor/src` wholesale with its `src`.
3. Reapply each patch, checking as you go that it is still needed:

   ```sh
   patch -p0 vendor/src/webview.rs -i /tmp/fork/webview.rs.patch
   ```

   `app.rs` and `config.rs` are the ones most likely to have been fixed
   upstream. A patch that no longer applies, or applies to code that already
   reads that way, is a divergence you get to drop. That is a win, not a
   problem: delete its entry from `UPSTREAM.toml` too.
4. Bump `version` in both `UPSTREAM.toml` and `vendor/Cargo.toml`.
5. Run the check, then `cargo test --release --workspace`.
6. Open twenty windows and confirm there is still one `WebKitWebProcess`. The
   divergence that matters is not covered by any unit test, because it is a
   property of the process tree and not of this crate's API. `harness/` has the
   rig for it.

Steps 1 and 3 are exact inverses, so on an unchanged upstream they reproduce
`vendor/src` byte for byte. That is worth knowing when a patch conflicts: the
conflict is upstream's change meeting ours, never a mistake in the extraction.

Step 6 is the one to not skip. Upstream reworking how a `WebContext` is built
would compile cleanly here and silently restore one network process per window,
which is the whole thing this fork exists to prevent.

## When this goes away

When the shared-process behaviour lands upstream. The fix belongs there: the
custom protocol is registered per `WebContext`, and ordering that registration
against a webview id is something only upstream can do. Once upstream can share
a context safely, vitrum drops this crate and depends on `dioxus-desktop`
again.
