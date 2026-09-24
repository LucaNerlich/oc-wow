#!/usr/bin/env python3
"""Independently validate a generated OCWow reply font using macOS CoreText.

This is a development aid, not part of the runtime. It loads the font with the
system font engine (via CoreText/CoreGraphics) and prints glyph ids and advance
widths, verifying that the hand-rolled TrueType tables are structurally valid
and that advance widths encode the expected bytes.

Usage:
    python3 tools/validate_font.py path/to/fontreply0001.ttf
"""

import ctypes as C
import sys

CT = "/System/Library/Frameworks/CoreText.framework/CoreText"
CF = "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation"


class CGSize(C.Structure):
    _fields_ = [("width", C.c_double), ("height", C.c_double)]


def setup():
    ct = C.cdll.LoadLibrary(CT)
    cf = C.cdll.LoadLibrary(CF)

    cf.CFURLCreateFromFileSystemRepresentation.restype = C.c_void_p
    cf.CFURLCreateFromFileSystemRepresentation.argtypes = [
        C.c_void_p,
        C.c_char_p,
        C.c_long,
        C.c_bool,
    ]
    cf.CFArrayGetCount.restype = C.c_long
    cf.CFArrayGetCount.argtypes = [C.c_void_p]
    cf.CFArrayGetValueAtIndex.restype = C.c_void_p
    cf.CFArrayGetValueAtIndex.argtypes = [C.c_void_p, C.c_long]
    cf.CFRelease.argtypes = [C.c_void_p]

    ct.CTFontManagerCreateFontDescriptorsFromURL.restype = C.c_void_p
    ct.CTFontManagerCreateFontDescriptorsFromURL.argtypes = [C.c_void_p]
    ct.CTFontCreateWithFontDescriptor.restype = C.c_void_p
    ct.CTFontCreateWithFontDescriptor.argtypes = [C.c_void_p, C.c_double, C.c_void_p]
    ct.CTFontGetUnitsPerEm.restype = C.c_uint
    ct.CTFontGetUnitsPerEm.argtypes = [C.c_void_p]
    ct.CTFontGetGlyphsForCharacters.restype = C.c_bool
    ct.CTFontGetGlyphsForCharacters.argtypes = [
        C.c_void_p,
        C.POINTER(C.c_uint16),
        C.POINTER(C.c_uint16),
        C.c_long,
    ]
    ct.CTFontGetAdvancesForGlyphs.restype = C.c_double
    ct.CTFontGetAdvancesForGlyphs.argtypes = [
        C.c_void_p,
        C.c_int,
        C.POINTER(C.c_uint16),
        C.POINTER(CGSize),
        C.c_long,
    ]
    return ct, cf


def main(path: str) -> int:
    ct, cf = setup()

    raw = path.encode()
    url = cf.CFURLCreateFromFileSystemRepresentation(None, raw, len(raw), False)
    if not url:
        print(f"FAIL: could not build a URL for {path}")
        return 1

    descriptors = ct.CTFontManagerCreateFontDescriptorsFromURL(url)
    cf.CFRelease(url)
    if not descriptors:
        print(f"FAIL: the system font engine rejected {path}")
        return 1

    count = cf.CFArrayGetCount(descriptors)
    if count < 1:
        print("FAIL: no font descriptors")
        return 1
    descriptor = cf.CFArrayGetValueAtIndex(descriptors, 0)

    font = ct.CTFontCreateWithFontDescriptor(descriptor, 64.0, None)
    if not font:
        print("FAIL: could not instantiate the font")
        return 1

    print(f"font:  {path}")
    print(f"upem:  {ct.CTFontGetUnitsPerEm(font)}")

    def measure(codepoint: int):
        ch = C.c_uint16(codepoint)
        glyph = C.c_uint16(0)
        if not ct.CTFontGetGlyphsForCharacters(font, C.byref(ch), C.byref(glyph), 1):
            return None, None
        size = CGSize()
        ct.CTFontGetAdvancesForGlyphs(font, 0, C.byref(glyph), C.byref(size), 1)
        return glyph.value, size.width

    def report(label, codepoint):
        glyph, width = measure(codepoint)
        print(f"  {label:12s} U+{codepoint:04X}  glyph={glyph}  advance={width}")

    report("trail", 0x007E)
    report("cal low", 0xE200)
    report("cal high", 0xE201)
    for i in range(4):
        report(f"data[{i}]", 0xE000 + i)

    low = measure(0xE200)[1]
    high = measure(0xE201)[1]
    if low is None or high is None or high <= low:
        print("FAIL: calibration glyphs are not usable")
        return 1
    print(f"calibration span: {high - low:.1f} px at 64pt")

    cf.CFRelease(descriptors)
    print("OK: font is structurally valid and measurable")
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print(__doc__)
        sys.exit(2)
    sys.exit(main(sys.argv[1]))
