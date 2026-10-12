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

Measured against feh, mpv and VLC on a 4K display (i7-12700K, X11 with
XFCE), each viewer fullscreen, reading the screen itself to see when the
picture is there. "Start" is from launching the process to the first
image on screen; "next" is from the key press to the next image on
screen, median of 18 presses. Lower is better.

| Image | Metric | jog | feh | mpv | VLC |
|---|---|---|---|---|---|
| 1080p JPEG | start | 133 ms | 85 ms | 415 ms | 574 ms |
| | next | 33 ms | 36 ms | 47 ms | 47 ms |
| 4K JPEG | start | 121 ms | 142 ms | 441 ms | 577 ms |
| | next | 34 ms | 69 ms | 67 ms | 64 ms |
| 24-megapixel JPEG | start | 184 ms | 225 ms | 567 ms | 712 ms |
| | next | 29 ms | 189 ms | 182 ms | 165 ms |

The next image takes the same ~30 ms whatever its size, because it was
decoded while you looked at the last one. Run with `--debug` to see
every decode, frame and wait timed on your own machine.
