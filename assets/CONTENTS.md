# What is in these files

Every image this repository publishes, what it shows, and a digest of the bytes
that showed it.

A picture of vitrum shows coding agents in the states only vitrum surfaces: an
agent working, an agent blocked on approval, an agent that finished while you
were looking elsewhere, several projects at once. Each entry names the agents on
screen and the state each one is in, so the claim can be read against the
picture without opening a review.

Brand marks are not session views, and say so instead.

The digest is FNV-1a over the file's bytes. It is here so that replacing a
picture is not a silent act: regenerating a screenshot changes the digest, the
build fails, and whoever regenerated it has to look at the new one and say what
is in it. Adding an image without adding its line fails the build too.

- assets/screenshots/hero.png @ c9a547ef465213eb: claude asking to apply an edit, focused and waiting on approval; gemini working; codex ready; claude waiting on input over in kernel-notes; four projects stacked in the sidebar
- assets/logo/vitrum.svg @ cd46b04b9fa01b11: brand mark, the vitrum glass wordmark
- assets/logo/vitrum-inverted.svg @ dca168329244632d: brand mark, the wordmark for dark backgrounds
- assets/logo/vitrum.ico @ ee390289cf4dc8a4: brand mark, the multi-resolution Windows icon
- assets/logo/vitrum-16.png @ d3e9833ee72affd7: brand mark, the window and tray icon at 16 px
- assets/logo/vitrum-24.png @ e1d338a58871c4f7: brand mark, the window and tray icon at 24 px
- assets/logo/vitrum-32.png @ 40b28592f9340a77: brand mark, the window and tray icon at 32 px
- assets/logo/vitrum-48.png @ 8181950fbd72f400: brand mark, the window and tray icon at 48 px
- assets/logo/vitrum-64.png @ 303e3b4280644363: brand mark, the window and tray icon at 64 px
- assets/logo/vitrum-128.png @ 62809601c9ff6429: brand mark, the application icon at 128 px
- assets/logo/vitrum-256.png @ 100f233c0a3da232: brand mark, the application icon at 256 px
- assets/logo/vitrum-512.png @ 20fc8af1d7bf136c: brand mark, the application icon at 512 px
- assets/logo/vitrum-1024.png @ b96d8fa3bc2bbbd5: brand mark, the application icon at 1024 px
