# jog

A fast image and video viewer for Linux. Everything on the way to the
screen is built for speed: images are decoded ahead of you on every
core, frames come out at the size of your screen so showing one is a
copy, and the thread that answers your keys never waits on a decode, a
resize or the video player.

```
jog PHOTOS/            # a folder, fullscreen
jog -r PHOTOS/         # and its subfolders
jog clip.mp4           # videos play through libmpv
```

## Speed

Measured on a 4K display with an i7-12700K, against jog before its
optimisation pass. Every figure is the same key script run on both
versions, alternating, on a quiet machine.

| What | Metric | Before | Now |
|---|---|---|---|
| Cold start, 1080p JPEG | process start to first frame | 90 ms | 53 ms |
| Cold start, 1440p PNG | process start to first frame | 94 ms | 54 ms |
| Open a folder whose first file is a video | process start to first frame | 585 ms | 194 ms |
| Step to an image already decoded | key press to frame, median | 18.7 ms | 9.3 ms |
| Step every 60 ms through fresh files | key press to frame, median | 15.4 ms | 5.8 ms |
| | key press to frame, p90 | 27.3 ms | 9.7 ms |
| | key press to frame, worst | 545 ms | 102 ms |
| Zoom and pan a 32-megapixel photo | redraw while moving | 17.3 ms | 2.6 ms |
| Grid of thumbnails | redraw while scrolling | 10.7 ms | 8.2 ms |
| Start a video | file open to first frame | 88 ms | 62 ms |
| Quit while a video plays | time to exit | ~300 ms | ~70 ms |

On a four-core machine the same work gives a 1.3x to 2x speedup
instead; run with `--debug` to see every decode, frame and wait timed
on your own.
