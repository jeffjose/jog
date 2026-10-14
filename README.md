# jog

A fast image and video viewer for Linux. Every step between the file and
the screen is built for speed:

- The first image starts decoding before the display is opened, and is
  shown as soon as the window appears.
- Upcoming images are decoded ahead of time on every core.
- Frames are decoded at the size of your screen, so showing one is a
  plain copy.
- The thread that handles your key presses never waits on a decode, a
  resize or the video player.

```
jog PHOTOS/            # a folder, fullscreen
jog -r PHOTOS/         # and its subfolders
jog clip.mp4           # videos play through libmpv
```

## Speed

Measured against feh, mpv and VLC on a 4K display (i7-12700K, X11 with
XFCE), with each viewer fullscreen. Timings come from reading the screen
itself to detect when the picture appears. "Start" is the time from
launching the process to the first image on screen; "next" is the time
from a key press to the next image on screen (median of 24 presses).
Lower is better.

| Image | Metric | jog | feh | mpv | VLC |
|---|---|---|---|---|---|
| 1080p JPEG | start | 95 ms | 51 ms | 402 ms | 532 ms |
| | next | 29 ms | 41 ms | 45 ms | 51 ms |
| 4K JPEG | start | 99 ms | 126 ms | 421 ms | 572 ms |
| | next | 26 ms | 71 ms | 65 ms | 67 ms |
| 24-megapixel JPEG | start | 185 ms | 228 ms | 566 ms | 686 ms |
| | next | 27 ms | 181 ms | 177 ms | 159 ms |

The next image takes about 27 ms regardless of its size, because it was
decoded while you were looking at the previous one. At 1080p feh starts
faster because it shows the image at its original size in the middle of
the screen, whereas jog scales it to fill the screen.

To measure on your own machine (X11 or Wayland):

```
jog --benchmark PHOTOS/
```

The benchmark opens the folder and steps through it, first at a normal
viewing pace and then faster than the files can decode. It then goes
back, zooms, pages through the grid and starts a video if there is one.
Each step is timed, and the results are printed before it quits.
`--debug` prints every decode, frame and wait as it happens.
