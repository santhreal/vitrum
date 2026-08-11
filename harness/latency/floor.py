"""A window that repaints on every key, and nothing else.

The platform floor. Between a key being pressed and a pixel changing there is
a cost that belongs to the display server and not to any application: the
server delivers the event, the client is scheduled, the drawing request goes
back, and the result is composited. Any client on that display pays it.

Measuring a terminal without measuring this reports the floor as if it were
the terminal's own cost. Run the same probe against this window and the
difference is what the terminal actually spends.

The window draws one filled rectangle in a colour that changes on every key,
which is the least work an X client can do and still be visible to a pixel
probe.

Usage:
  floor.py --display :8 --geometry 1100x300
"""

import argparse
import ctypes
import ctypes.util
import time

KEY_PRESS_MASK = 1 << 0
EXPOSURE_MASK = 1 << 15
KEY_PRESS = 2
EXPOSE = 12


class XEvent(ctypes.Structure):
    """Only the tag is read here, so the body is opaque storage.

    An X event union is 192 bytes on 64-bit. Declaring it short would let the
    server write past the end of the allocation on the first event.
    """

    _fields_ = [("type", ctypes.c_int), ("pad", ctypes.c_long * 24)]


def library():
    """libX11 with the return and argument types this file needs.

    A pointer returned as a default C `int` is truncated to 32 bits, and the
    first call that uses it addresses memory that was never mapped. Every
    handle here is declared.
    """
    name = ctypes.util.find_library("X11")
    if not name:
        raise SystemExit("libX11 is not installed")
    x = ctypes.CDLL(name)
    x.XOpenDisplay.restype = ctypes.c_void_p
    x.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x.XDefaultScreen.restype = ctypes.c_int
    x.XDefaultScreen.argtypes = [ctypes.c_void_p]
    x.XRootWindow.restype = ctypes.c_ulong
    x.XRootWindow.argtypes = [ctypes.c_void_p, ctypes.c_int]
    x.XBlackPixel.restype = ctypes.c_ulong
    x.XBlackPixel.argtypes = [ctypes.c_void_p, ctypes.c_int]
    x.XCreateSimpleWindow.restype = ctypes.c_ulong
    x.XCreateSimpleWindow.argtypes = [
        ctypes.c_void_p,
        ctypes.c_ulong,
        ctypes.c_int,
        ctypes.c_int,
        ctypes.c_uint,
        ctypes.c_uint,
        ctypes.c_uint,
        ctypes.c_ulong,
        ctypes.c_ulong,
    ]
    x.XSelectInput.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_long]
    x.XMapWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
    x.XFlush.argtypes = [ctypes.c_void_p]
    x.XCreateGC.restype = ctypes.c_void_p
    x.XCreateGC.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_void_p]
    x.XSetForeground.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_ulong]
    x.XFillRectangle.argtypes = [
        ctypes.c_void_p,
        ctypes.c_ulong,
        ctypes.c_void_p,
        ctypes.c_int,
        ctypes.c_int,
        ctypes.c_uint,
        ctypes.c_uint,
    ]
    x.XNextEvent.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
    return x


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--display", default=":8")
    parser.add_argument("--geometry", default="1100x300")
    parser.add_argument("--seconds", type=float, default=600.0)
    args = parser.parse_args()

    width, height = (int(v) for v in args.geometry.lower().split("x"))
    x = library()
    dpy = x.XOpenDisplay(args.display.encode())
    if not dpy:
        raise SystemExit(f"no display {args.display}")

    screen = x.XDefaultScreen(dpy)
    root = x.XRootWindow(dpy, screen)
    black = x.XBlackPixel(dpy, screen)
    win = x.XCreateSimpleWindow(dpy, root, 0, 0, width, height, 0, black, black)
    x.XSelectInput(dpy, win, KEY_PRESS_MASK | EXPOSURE_MASK)
    x.XMapWindow(dpy, win)
    x.XFlush(dpy)

    gc = x.XCreateGC(dpy, win, 0, None)
    event = XEvent()
    frame = 0
    end = time.monotonic() + args.seconds

    while time.monotonic() < end:
        x.XNextEvent(dpy, ctypes.byref(event))
        if event.type not in (KEY_PRESS, EXPOSE):
            continue
        frame += 1
        # A colour that is new on every frame, so a probe watching for a change
        # cannot be answered by a repeat of what was already there.
        colour = (frame * 2654435761) & 0xFFFFFF
        x.XSetForeground(dpy, gc, colour)
        x.XFillRectangle(dpy, win, gc, 0, 0, width, height)
        x.XFlush(dpy)


if __name__ == "__main__":
    main()
