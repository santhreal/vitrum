# vitrum-dioxus-desktop

A fork of [`dioxus-desktop`](https://crates.io/crates/dioxus-desktop) 0.7.10 by
Jonathan Kelley, carried by [vitrum](https://github.com/santhreal/vitrum) under
the MIT licence. Everything except the change below is upstream's code.

## What diverges

One change, in `src/webview.rs`. On Linux each new webview is built with
`with_related_view`, pointed at a webview that is still alive, so every window
runs inside the first window's `WebKitWebProcess`. `LIVE_WEBKIT_VIEWS` tracks
which views are still real so the relation target is never a dead widget.

Upstream builds a fresh `WebContext` per webview, and on Linux each one starts
its own `WebKitNetworkProcess`. Measured on this project: 20 windows ran 20 of
them at 8.6 MB PSS each, so 171.2 MB went on twenty copies of one cache and
cookie jar. vitrum's memory target for 20 windows does not survive that.

## Why a fork and not a patch

`[patch.crates-io]` applies only to a workspace build. Anyone running
`cargo install vitrum` resolves the registry copy instead, so the published
client would have silently shipped the per-window process behaviour. A named
dependency is the only form of this change that survives publication.

## When this goes away

When the shared-process behaviour lands upstream. The fix belongs there: the
custom protocol is registered per `WebContext`, and ordering that registration
against a webview id is something only upstream can do. Once upstream can share
a context safely, vitrum drops this crate and depends on `dioxus-desktop`
again.
