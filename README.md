# jog

A fast image and video viewer for Linux. Everything on the way to the
screen is built for speed: the first image is decoding before the
display is even opened and goes up with the window itself, images are
decoded ahead of you on every core, frames come out at the size of your
screen so showing one is a copy, and the thread that answers your keys
never waits on a decode, a resize or the video player.

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
screen, median of 24 presses. Lower is better.

| Image | Metric | jog | feh | mpv | VLC |
|---|---|---|---|---|---|
| 1080p JPEG | start | 95 ms | 51 ms | 402 ms | 532 ms |
| | next | 29 ms | 41 ms | 45 ms | 51 ms |
| 4K JPEG | start | 99 ms | 126 ms | 421 ms | 572 ms |
| | next | 26 ms | 71 ms | 65 ms | 67 ms |
| 24-megapixel JPEG | start | 185 ms | 228 ms | 566 ms | 686 ms |
| | next | 27 ms | 181 ms | 177 ms | 159 ms |

The next image takes the same ~27 ms whatever its size, because it was
decoded while you looked at the last one. At 1080p feh starts quicker
because it shows the image at its own size in the middle of the screen,
where jog fills the screen with it. Run with `--debug` to see every
decode, frame and wait timed on your own machine.
