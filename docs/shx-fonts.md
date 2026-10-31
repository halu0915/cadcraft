# SHX stroke fonts

CADCraft reads user-supplied AutoCAD-86 shapes 1.0, unifont 1.0 and bigfont 1.0
files. There are no bundled SHX assets. The parser and interpreter use bounded
record sizes, a four-position stack, a 32-level subshape depth limit, and a
65,536-operation budget per character. Invalid files or glyph programs fall back
with a warning rather than partially drawing a broken glyph.

Set `FONTPATH` through `SETVAR`, or start with `CADCRAFT_FONTPATH` containing
platform-separated search directories. Absolute font paths work too. Search is
case insensitive within those directories. Setting FONTPATH clears cached fonts
and warnings, including missing-file entries. The cache has a 64 MiB program-byte
budget and at most 64 entries.

STYLE's `font` selects the regular face; `bigFont` selects an optional bigfont.
Bigfont escape ranges are checked before using encoded two-byte character IDs.
ASCII symbol escape pairs are supported as well. Set `CADCRAFT_SHX_ENCODING` to
`big5` (default) or `gbk`/`gb2312` for the legacy encoding. Ranges cannot reliably
identify a code page, so this choice is explicit and process-wide. SHX Unicode
fonts use their Unicode shape IDs directly.

TEXT captures STYLE's width factor and oblique when created, and thereafter uses
its entity values, including imported DXF values. MTEXT uses STYLE values plus
inline formatting. Font-unit advances, normalized by each font's own cap height,
are shared by measurement and rendering. SHX geometry remains vector strokes in
PDF and follows the same renderer for screen and PNG. Missing characters use the
existing built-in/installed fallback; warnings are deduplicated.

The `cadcraft-fonts` example `shx_measure` accepts two local font paths, height,
width factor and strings, and prints numeric advances for local acceptance. It
does not embed or copy font data. Tests construct synthetic font bytes in code.

Geometry references:

- [Autodesk Special Codes Reference](https://help.autodesk.com/cloudhelp/2022/ENU/AutoCAD-Customization/files/GUID-06832147-16BE-4A66-A6D0-3ADF98DC8228.htm)
- [Autodesk Extended Big Font definitions](https://help.autodesk.com/cloudhelp/2026/ENU/AutoCAD-MAC-Customization/files/GUID-00ED0CC6-A4BE-4591-93FA-598CC40AA43D.htm)

The implementation covers compact vectors, pen state, scaling, position stack,
subshapes, octant and fractional arcs, bulge arcs and horizontal skipping of
vertical-only commands. Vertical layout itself is not introduced by this change.
