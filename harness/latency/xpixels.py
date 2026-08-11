"""Pixel-level latency probe for an X client.

Measures the interval between a cause and the pixels that answer it, from
outside the program under test. Nothing is instrumented and no source is
needed, so the same probe measures a released binary and a development build
and the two figures mean the same thing.

Two clocks would make the numbers meaningless, so there is one: every timestamp
is CLOCK_MONOTONIC taken in this process, on the same host as the X server and
the program under test.

Requires a display it may type into. Never point it at a display someone is
using: it injects synthetic key events into whatever holds focus.

Only the standard library. The rig this runs on has no package manager access
worth relying on, and a probe that cannot be installed is a probe nobody runs.
"""

import ctypes
import ctypes.util
import time
import zlib

ZPIXMAP = 2
ALL_PLANES = (1 << 64) - 1
REVERT_TO_PARENT = 1
CURRENT_TIME = 0


DESTROY_IMAGE = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p)

# One foreign-function object per distinct entry point, built once. Building it
# per frame made the interpreter collect a callable whose backing buffer the
# call had just freed, which crashes inside the collector and not at the call.
_DESTROYERS = {}


def _destroyer(address):
    fn = _DESTROYERS.get(address)
    if fn is None:
        fn = DESTROY_IMAGE(address)
        _DESTROYERS[address] = fn
    return fn


class XImageFuncs(ctypes.Structure):
    """The function table hanging off an XImage.

    `XDestroyImage` is a macro in Xlib.h, not an exported symbol, so an image
    is released by calling the pointer it carries. Getting this wrong leaks a
    frame per sample, and a probe that polls a rectangle thousands of times a
    second exhausts the machine within the run.
    """

    _fields_ = [
        ("create_image", ctypes.c_void_p),
        ("destroy_image", ctypes.c_void_p),
        ("get_pixel", ctypes.c_void_p),
        ("put_pixel", ctypes.c_void_p),
        ("sub_image", ctypes.c_void_p),
        ("add_pixel", ctypes.c_void_p),
    ]


class XImage(ctypes.Structure):
    """Xlib's XImage, through the function table at its end."""

    _fields_ = [
        ("width", ctypes.c_int),
        ("height", ctypes.c_int),
        ("xoffset", ctypes.c_int),
        ("format", ctypes.c_int),
        ("data", ctypes.c_void_p),
        ("byte_order", ctypes.c_int),
        ("bitmap_unit", ctypes.c_int),
        ("bitmap_bit_order", ctypes.c_int),
        ("bitmap_pad", ctypes.c_int),
        ("depth", ctypes.c_int),
        ("bytes_per_line", ctypes.c_int),
        ("bits_per_pixel", ctypes.c_int),
        ("red_mask", ctypes.c_ulong),
        ("green_mask", ctypes.c_ulong),
        ("blue_mask", ctypes.c_ulong),
        ("obdata", ctypes.c_void_p),
        ("f", XImageFuncs),
    ]


class XWindowAttributes(ctypes.Structure):
    """Xlib's XWindowAttributes, in full.

    In full and not up to the field of interest: `XGetWindowAttributes` writes
    the whole structure, so a short definition is a heap overflow. It does not
    fail at the call; it fails later, inside the garbage collector, and looks
    like anything but a struct definition.
    """

    _fields_ = [
        ("x", ctypes.c_int),
        ("y", ctypes.c_int),
        ("width", ctypes.c_int),
        ("height", ctypes.c_int),
        ("border_width", ctypes.c_int),
        ("depth", ctypes.c_int),
        ("visual", ctypes.c_void_p),
        ("root", ctypes.c_ulong),
        ("c_class", ctypes.c_int),
        ("bit_gravity", ctypes.c_int),
        ("win_gravity", ctypes.c_int),
        ("backing_store", ctypes.c_int),
        ("backing_planes", ctypes.c_ulong),
        ("backing_pixel", ctypes.c_ulong),
        ("save_under", ctypes.c_int),
        ("colormap", ctypes.c_ulong),
        ("map_installed", ctypes.c_int),
        ("map_state", ctypes.c_int),
        ("all_event_masks", ctypes.c_long),
        ("your_event_mask", ctypes.c_long),
        ("do_not_propagate_mask", ctypes.c_long),
        ("override_redirect", ctypes.c_int),
        ("screen", ctypes.c_void_p),
    ]


def now():
    """CLOCK_MONOTONIC seconds. The only clock in this file."""
    return time.clock_gettime(time.CLOCK_MONOTONIC)


class Screen:
    """A display, the window under test, and a rectangle sampled from it."""

    def __init__(self, display=None):
        x11 = ctypes.util.find_library("X11")
        xtst = ctypes.util.find_library("Xtst")
        if not x11:
            raise SystemExit("libX11 is not installed")
        self.x = ctypes.CDLL(x11)
        self.xtst = ctypes.CDLL(xtst) if xtst else None

        self.x.XOpenDisplay.restype = ctypes.c_void_p
        self.x.XOpenDisplay.argtypes = [ctypes.c_char_p]
        self.x.XGetImage.restype = ctypes.POINTER(XImage)
        self.x.XGetImage.argtypes = [
            ctypes.c_void_p,
            ctypes.c_ulong,
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_uint,
            ctypes.c_uint,
            ctypes.c_ulong,
            ctypes.c_int,
        ]
        self.x.XDefaultRootWindow.restype = ctypes.c_ulong
        self.x.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
        self.x.XStringToKeysym.restype = ctypes.c_ulong
        self.x.XStringToKeysym.argtypes = [ctypes.c_char_p]
        self.x.XKeysymToKeycode.restype = ctypes.c_ubyte
        self.x.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
        self.x.XSetInputFocus.argtypes = [
            ctypes.c_void_p,
            ctypes.c_ulong,
            ctypes.c_int,
            ctypes.c_ulong,
        ]
        self.x.XFlush.argtypes = [ctypes.c_void_p]
        self.x.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
        self.x.XQueryTree.argtypes = [
            ctypes.c_void_p,
            ctypes.c_ulong,
            ctypes.POINTER(ctypes.c_ulong),
            ctypes.POINTER(ctypes.c_ulong),
            ctypes.POINTER(ctypes.POINTER(ctypes.c_ulong)),
            ctypes.POINTER(ctypes.c_uint),
        ]
        self.x.XGetWindowAttributes.argtypes = [
            ctypes.c_void_p,
            ctypes.c_ulong,
            ctypes.POINTER(XWindowAttributes),
        ]
        self.x.XFree.argtypes = [ctypes.c_void_p]
        if self.xtst:
            self.xtst.XTestFakeKeyEvent.argtypes = [
                ctypes.c_void_p,
                ctypes.c_uint,
                ctypes.c_int,
                ctypes.c_ulong,
            ]
            self.xtst.XTestFakeButtonEvent.argtypes = [
                ctypes.c_void_p,
                ctypes.c_uint,
                ctypes.c_int,
                ctypes.c_ulong,
            ]
            self.xtst.XTestFakeMotionEvent.argtypes = [
                ctypes.c_void_p,
                ctypes.c_int,
                ctypes.c_int,
                ctypes.c_int,
                ctypes.c_ulong,
            ]

        name = display.encode() if display else None
        self.dpy = self.x.XOpenDisplay(name)
        if not self.dpy:
            raise SystemExit(f"cannot open display {display}")
        self.root = self.x.XDefaultRootWindow(self.dpy)

    def geometry(self, window):
        """`(x, y, width, height)` of `window` in its own coordinates."""
        attrs = XWindowAttributes()
        self.x.XGetWindowAttributes(self.dpy, window, ctypes.byref(attrs))
        return attrs.x, attrs.y, attrs.width, attrs.height

    def biggest_child(self):
        """The largest mapped top-level window, which is the one under test.

        Xvfb has no window manager, so a top-level window is a direct child of
        the root and there is no frame to see past.
        """
        root_ret = ctypes.c_ulong()
        parent = ctypes.c_ulong()
        children = ctypes.POINTER(ctypes.c_ulong)()
        count = ctypes.c_uint()
        ok = self.x.XQueryTree(
            self.dpy,
            self.root,
            ctypes.byref(root_ret),
            ctypes.byref(parent),
            ctypes.byref(children),
            ctypes.byref(count),
        )
        if not ok:
            return None
        best, best_area = None, 0
        for i in range(count.value):
            win = children[i]
            attrs = XWindowAttributes()
            self.x.XGetWindowAttributes(self.dpy, win, ctypes.byref(attrs))
            if attrs.map_state != 2:
                continue
            area = attrs.width * attrs.height
            if area > best_area:
                best, best_area = win, area
        if children:
            self.x.XFree(children)
        return best

    def grab(self, rect):
        """The raw bytes of `rect` on the root window.

        Root rather than the window itself: Xvfb draws every window into the
        root's frame buffer with no compositing redirection, so this reads what
        a camera pointed at the screen would see, including anything drawn over
        the program under test.
        """
        x, y, w, h = rect
        img = self.x.XGetImage(
            self.dpy, self.root, x, y, w, h, ALL_PLANES, ZPIXMAP
        )
        if not img:
            raise RuntimeError("XGetImage returned nothing")
        head = img.contents
        raw = ctypes.string_at(head.data, head.bytes_per_line * head.height)
        _destroyer(head.f.destroy_image)(ctypes.cast(img, ctypes.c_void_p))
        return raw

    def digest(self, rect):
        """A cheap hash of `rect`, for "did anything change"."""
        return zlib.crc32(self.grab(rect))

    def colours(self, rect):
        """How many distinct 32-bit pixel values `rect` holds.

        One colour is a blank surface. A handful is a background and a border.
        Text pushes it into the dozens through antialiasing, which is what
        makes this the test for a frame being readable rather than merely
        painted.
        """
        raw = self.grab(rect)
        return len({raw[i : i + 4] for i in range(0, len(raw) - 3, 4)})

    def focus(self, window):
        self.x.XSetInputFocus(self.dpy, window, REVERT_TO_PARENT, CURRENT_TIME)
        self.x.XSync(self.dpy, 0)

    def keycode(self, name):
        sym = self.x.XStringToKeysym(name.encode())
        if sym == 0:
            raise SystemExit(f"no keysym named {name}")
        code = self.x.XKeysymToKeycode(self.dpy, sym)
        if code == 0:
            raise SystemExit(f"no keycode for {name}")
        return code

    def press(self, code):
        """Type one key, and return the moment the request left this process.

        `XFlush` before the timestamp would not help: the interval that matters
        starts when the server can first see the event, and that is when the
        write to the socket has been made.
        """
        if not self.xtst:
            raise SystemExit("libXtst is not installed, so no key can be typed")
        self.xtst.XTestFakeKeyEvent(self.dpy, code, 1, 0)
        self.xtst.XTestFakeKeyEvent(self.dpy, code, 0, 0)
        self.x.XFlush(self.dpy)
        return now()

    def click(self, x, y, button=1):
        """Move the pointer to `x, y` and click. Returns when the request left.

        A pointer and not only a keyboard because the sidebar is the product:
        selecting a session is a click on a row, and a probe that can only type
        cannot reach the surface the whole window is arranged around.
        """
        if not self.xtst:
            raise SystemExit("libXtst is not installed, so nothing can be clicked")
        self.xtst.XTestFakeMotionEvent(self.dpy, -1, x, y, 0)
        self.x.XFlush(self.dpy)
        self.xtst.XTestFakeButtonEvent(self.dpy, button, 1, 0)
        self.xtst.XTestFakeButtonEvent(self.dpy, button, 0, 0)
        self.x.XFlush(self.dpy)
        return now()

    def chord(self, modifiers, name):
        """Hold `modifiers`, tap `name`, release. Returns when the request left.

        A chord and not a key because a pane is reached by one: the sessions a
        window holds are selected with Alt and a digit, and a probe that could
        only tap unmodified keys could not put a session on screen to measure.
        """
        if not self.xtst:
            raise SystemExit("libXtst is not installed, so no key can be typed")
        codes = [self.keycode(m) for m in modifiers]
        key = self.keycode(name)
        for code in codes:
            self.xtst.XTestFakeKeyEvent(self.dpy, code, 1, 0)
        self.xtst.XTestFakeKeyEvent(self.dpy, key, 1, 0)
        self.xtst.XTestFakeKeyEvent(self.dpy, key, 0, 0)
        for code in reversed(codes):
            self.xtst.XTestFakeKeyEvent(self.dpy, code, 0, 0)
        self.x.XFlush(self.dpy)
        return now()

    def wait_change(self, rect, before, timeout):
        """Poll until `rect` differs from `before`, and return when it did.

        Returns `None` on timeout rather than raising, so a caller can count a
        miss and carry on: one dropped sample is data, and an exception here
        would throw away the samples already taken.
        """
        deadline = now() + timeout
        while True:
            current = self.digest(rect)
            if current != before:
                return now(), current
            if now() > deadline:
                return None
