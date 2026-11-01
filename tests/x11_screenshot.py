"""Capture an explicit X11 test display using the GTK libraries required by Tauri.

WebKitGTK's WebDriver screenshot waits for a presentation update, which can
stall under Xvfb. This captures the actual rendered X11 window independently.
Only used when OPENREVO_TEST_SCREENSHOT_BACKEND=x11 is explicitly selected.
"""
import ctypes as c
from ctypes.util import find_library
import os


def capture(rect):
    if not os.environ.get('DISPLAY'):
        raise RuntimeError('X11 screenshot requires an explicit test DISPLAY')
    gdk = c.CDLL(find_library('gdk-3'))
    pixbuf = c.CDLL(find_library('gdk_pixbuf-2.0'))
    glib = c.CDLL(find_library('glib-2.0'))
    gobject = c.CDLL(find_library('gobject-2.0'))

    def function(library, name, result, *arguments):
        method = getattr(library, name)
        method.restype = result
        method.argtypes = list(arguments)
        return method

    function(gdk, 'gdk_set_allowed_backends', None, c.c_char_p)(b'x11')
    if not function(gdk, 'gdk_init_check', c.c_int, c.c_void_p, c.c_void_p)(None, None):
        raise RuntimeError('Cannot connect to the X11 test display')
    root = function(gdk, 'gdk_get_default_root_window', c.c_void_p)()
    if not root:
        raise RuntimeError('X11 test root window is unavailable')
    root_width = function(gdk, 'gdk_window_get_width', c.c_int, c.c_void_p)(root)
    root_height = function(gdk, 'gdk_window_get_height', c.c_int, c.c_void_p)(root)
    x, y, width, height = (int(rect[key]) for key in ('x', 'y', 'width', 'height'))
    if x < 0 or y < 0 or width <= 0 or height <= 0 or x + width > root_width or y + height > root_height:
        raise RuntimeError(f'Test window is outside the X11 capture area: {rect}')
    picture = function(gdk, 'gdk_pixbuf_get_from_window', c.c_void_p,
                       c.c_void_p, c.c_int, c.c_int, c.c_int, c.c_int)(root, x, y, width, height)
    if not picture:
        raise RuntimeError('Could not capture the X11 test window')
    buffer = c.c_void_p()
    length = c.c_size_t()
    error = c.c_void_p()
    try:
        channels = function(pixbuf, 'gdk_pixbuf_get_n_channels', c.c_int, c.c_void_p)(picture)
        stride = function(pixbuf, 'gdk_pixbuf_get_rowstride', c.c_int, c.c_void_p)(picture)
        pixels_pointer = function(pixbuf, 'gdk_pixbuf_get_pixels', c.c_void_p, c.c_void_p)(picture)
        pixels = c.string_at(pixels_pointer, (height - 1) * stride + width * channels)
        colors = {pixels[row * stride + column * channels:row * stride + column * channels + 3]
                  for row in range(0, height, max(1, height // 32))
                  for column in range(0, width, max(1, width // 32))}
        if len(colors) < 2:
            raise RuntimeError('X11 screenshot is blank; verify the app uses the test DISPLAY')
        saved = function(pixbuf, 'gdk_pixbuf_save_to_bufferv', c.c_int,
                         c.c_void_p, c.POINTER(c.c_void_p), c.POINTER(c.c_size_t),
                         c.c_char_p, c.c_void_p, c.c_void_p, c.POINTER(c.c_void_p))(
                             picture, c.byref(buffer), c.byref(length), b'png', None, None, c.byref(error))
        if not saved or not buffer.value:
            raise RuntimeError('Could not encode the X11 test screenshot')
        return c.string_at(buffer, length.value)
    finally:
        if buffer.value:
            function(glib, 'g_free', None, c.c_void_p)(buffer)
        if error.value:
            function(glib, 'g_error_free', None, c.c_void_p)(error)
        function(gobject, 'g_object_unref', None, c.c_void_p)(picture)
