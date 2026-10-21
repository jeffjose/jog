"""Builds an icon font for jog from Solar's linear icons, drawn at a
heavier line than Solar's own.

    uv run --no-project --with fonttools --with picosvg \
        tools/solar_font.py ICONS_JSON tools/solar_icons.txt assets/solar.ttf [STROKE]

ICONS_JSON is the set as Iconify publishes it:
https://cdn.jsdelivr.net/npm/@iconify-json/solar/icons.json

tools/solar_icons.txt has a line for each icon jog uses: the name of its
constant in `commands::icons`, and the code point the font has it at, in
hex. To add an icon, give it a line there with a code point of its own
and say in PICKS below which of Solar's drawings it is, or draw it in OWN.

The line is 2.25 wide where Solar draws it at 1.5: beside text at 13 to
24 pixels the thinner line is lighter than the letters.
"""

import json
import re
import sys

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.cu2quPen import Cu2QuPen
from fontTools.pens.transformPen import TransformPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.svgLib.path import parse_path
from picosvg.svg import SVG

# jog's name for an icon, and the Solar drawing that stands for it.
PICKS = {
    "ARROW_RIGHT": "arrow-right", "ARROW_LEFT": "arrow-left", "SEARCH": "magnifier", "X": "close",
    "SHUFFLE": "shuffle", "A_TO_Z": "sort-alphabetically", "CLOCK": "clock-circle", "WEIGHT": "scale",
    "PLAY": "play", "PAUSE": "pause", "ROTATE_CCW": "undo-left-round", "ROTATE_CW": "undo-right-round",
    "CHEVRONS_RIGHT": "double-alt-arrow-right", "CHEVRONS_LEFT": "double-alt-arrow-left",
    "GAUGE": "speedometer-middle", "VOLUME_2": "volume-loud", "VOLUME_1": "volume-small",
    "VOLUME_X": "volume-cross", "REPEAT": "repeat", "REPEAT_1": "repeat-one", "CLAPPERBOARD": "clapperboard",
    "CAPTIONS": "subtitles", "CAPTIONS_OFF": "text-cross", "AUDIO_LINES": "soundwave",
    "A_LARGE_SMALL": "text-field", "ZOOM_IN": "magnifier-zoom-in", "ZOOM_OUT": "magnifier-zoom-out",
    "SCAN": "scanner", "MAXIMIZE": "maximize", "EXPAND": "full-screen", "SHRINK": "quit-full-screen",
    "FULLSCREEN": "full-screen-square", "FLIP_VERTICAL": "flip-vertical", "FLIP_HORIZONTAL": "flip-horizontal",
    "LAYOUT_GRID": "widget", "FILM": "video-frame", "LIST": "list", "IMAGES": "gallery", "EYE": "eye",
    "INFO": "info-circle", "CAMERA": "camera", "ARROW_UP_DOWN": "sort-vertical",
    "SORT": "sort-from-top-to-bottom", "STAR": "star", "STAR_OFF": "star-off", "FLAG": "flag", "COPY": "copy",
    "BRACES": "braces", "FOLDER_INPUT": "move-to-folder", "FOLDER_PLUS": "add-folder", "FOLDERS": "folders",
    "PENCIL": "pen", "REFRESH": "refresh", "UNDO": "undo-left", "CLIPBOARD": "clipboard", "MINUS": "minus",
    "TRASH": "trash-bin-minimalistic", "CIRCLE_X": "close-circle", "BUG": "bug", "TERMINAL": "command",
    "KEYBOARD": "keyboard", "LOG_OUT": "logout-2", "EXTERNAL_LINK": "square-arrow-right-up", "POP_IN": "square-arrow-left-down", "CHECK": "check",
    "CHEVRON_RIGHT": "alt-arrow-right", "CHEVRON_LEFT": "alt-arrow-left", "TRIANGLE_ALERT": "danger-triangle",
    "TYPE": "text-bold", "MONITOR_PLAY": "display", "FILE": "file", "STEP_FORWARD": "round-alt-arrow-right",
    "STEP_BACK": "round-alt-arrow-left", "FAST_FORWARD": "rewind-forward", "REWIND": "rewind-back",
    "SKIP_BACK": "skip-previous", "SKIP_FORWARD": "skip-next", "REPLACE": "transfer-horizontal",
    "LIST_FILTER": "filter", "LIST_X": "filter-close", "PIN": "pin",
    # Edit's tools, as hotshot has them.
    "SELECT": "cursor", "ARROW_UP_RIGHT": "arrow-right-up", "MARKER": "paint-roller",
    "SPOTLIGHT": "flashlight-on", "CROP": "crop", "EDIT": "pen-new-square", "REDO": "undo-right",
    "STICKER": "smile-circle",
    # Filled ones, taken as Solar draws them: named in full, style and all.
    "CHECK_FILLED": "check-circle-bold", "X_FILLED": "close-circle-bold",
}

LINE = 'fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"'

# Drawings of jog's own, on Solar's 24 by 24 square and in its manner: the
# plain shapes Edit draws, which Solar has no icons of. hotshot's, as they are.
OWN = {
    "LINE": f'<path d="M5 19L19 5" {LINE}/>',
    "RECT": f'<rect x="3" y="5" width="18" height="14" rx="4" {LINE}/>',
    "OVAL": f'<ellipse cx="12" cy="12" rx="9.25" ry="7.25" {LINE}/>',
    "COUNTER": f'<circle cx="12" cy="12" r="9.25" {LINE}/><path d="M10.25 9.75L12.75 8V16" {LINE}/>',
    # A mosaic: what a pixelated patch looks like.
    "BLUR": (
        f'<rect x="3" y="3" width="18" height="18" rx="4.5" {LINE}/>'
        '<rect x="7" y="7" width="3.4" height="3.4" rx="0.6" fill="currentColor"/>'
        '<rect x="13.6" y="7" width="3.4" height="3.4" rx="0.6" fill="currentColor"/>'
        '<rect x="10.3" y="10.3" width="3.4" height="3.4" rx="0.6" fill="currentColor"/>'
        '<rect x="7" y="13.6" width="3.4" height="3.4" rx="0.6" fill="currentColor"/>'
        '<rect x="13.6" y="13.6" width="3.4" height="3.4" rx="0.6" fill="currentColor"/>'
    ),
}

EM = 1000
GRID = 24


def outline(body, stroke):
    """The filled outline of a drawing made of lines `stroke` wide, as a
    glyph: Solar's 24 by 24 square taken to the font's em, the right way up."""
    body = re.sub(r'stroke-width="[0-9.]+"', f'stroke-width="{stroke}"', body).replace("currentColor", "#000")
    svg = SVG.fromstring(f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {GRID} {GRID}">{body}</svg>')
    pen = TTGlyphPen(None)
    scale = EM / GRID
    to_em = TransformPen(Cu2QuPen(pen, max_err=1.0, reverse_direction=False), (scale, 0, 0, -scale, 0, EM))
    for shape in svg.topicosvg().shapes():
        parse_path(shape.as_path().d, to_em)
    return pen.glyph()


def main():
    icons_json, consts, out = sys.argv[1:4]
    stroke = sys.argv[4] if len(sys.argv) > 4 else "2.25"
    solar = json.load(open(icons_json))["icons"]

    glyphs, cmap, missing = {".notdef": TTGlyphPen(None).glyph()}, {}, []
    for line in open(consts):
        name, point = line.split()
        pick = PICKS.get(name, "")
        drawing = OWN.get(name) or (solar.get(pick if pick.endswith("-bold") else pick + "-linear") or {}).get("body")
        if not drawing:
            missing.append(f"{name} ({PICKS.get(name, 'no pick')})")
            continue
        glyphs[name] = outline(drawing, stroke)
        cmap[int(point, 16)] = name
    if missing:
        sys.exit(f"Solar has no drawing for: {', '.join(missing)}")

    font = FontBuilder(EM, isTTF=True)
    font.setupGlyphOrder(list(glyphs))
    font.setupCharacterMap(cmap)
    font.setupGlyf(glyphs)
    font.setupHorizontalMetrics({name: (EM, 0) for name in glyphs})
    font.setupHorizontalHeader(ascent=EM, descent=0)
    font.setupNameTable({"familyName": "jog icons", "styleName": "Regular"})
    font.setupOS2(sTypoAscender=EM, sTypoDescender=0, usWinAscent=EM, usWinDescent=0)
    font.setupPost()
    font.save(out)
    print(f"{len(glyphs) - 1} icons")


if __name__ == "__main__":
    main()
