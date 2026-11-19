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

Videos:

- Played by mpv on the GPU; controls drawn over them in about 2 ms.
- Resume from the nearest keyframe.
- The next video is loaded while this one plays.
- Each player is its own process; a decoder crash falls back to the CPU.
- Stays responsive under heavy CPU load.
- `g` shows a folder as a grid with names; `G` as a wall, packed edge to
  edge with no names and every video on it playing. On the wall, `Space`
  turns the pages by themselves.

```sh
jog PHOTOS/            # a folder, fullscreen
jog -r PHOTOS/         # and its subfolders
jog clip.mp4           # videos play through libmpv
jog https://…          # a picture, a video, or a page with one (yt-dlp)
jog skyfall:.          # this folder, on skyfall, over ssh
```

## Over ssh

`host:path` is a folder or file on another machine. The path is read as
it would be here, then looked up there: standing in `/home/you/photos`,
`skyfall:.` is `/home/you/photos` on skyfall, `skyfall:../videos` is
`/home/you/videos`, `skyfall:~/x` is `x` in your home there and
`skyfall:/srv` is `/srv`. That is not scp's reading, where `skyfall:x`
starts at the remote home.

`host` is whatever `ssh host` takes, so `~/.ssh/config` aliases, keys and
jump hosts apply; `user@host:` names the login. `JOG_SSH='ssh -p 2222'`
changes the ssh command.

The host's `/` is mounted with sshfs (or rclone, without it) under
`$XDG_RUNTIME_DIR/jog/mnt/HOST`, and from there it is a folder like any
other: browsed, played, moved, renamed and converted in place. Each
jog open on a host shares its mount, and the last one to quit unmounts
it. Paths are shown and copied as `skyfall:/srv/a.jpg`, Move to… takes
`skyfall:/srv/` for a host already open, and trash is the host's own.

With jog installed on the host too, thumbnails are made there: jog starts
`ssh host jog --serve` beside the mount, and only the thumbnail crosses
the network, not the file. The first screen of a grid of 105 24-megapixel
photos over a 32 MB/s link: 22.6 s through the mount alone, 5.9 s with
it. Nothing waits on it; without jog there, an older one, or a dropped
connection, thumbnails are read through the mount. `JOG_REMOTE` names
the jog over there when it is not on its `PATH`; `--debug` says how it
fares.

## Speed

Measured against feh, mpv and VLC on a 4K display (i7-12700K, X11 with
XFCE), with each viewer fullscreen. Timings come from reading the screen
itself to detect when the picture appears. "Start" is the time from
launching the process to the first image on screen; "next" is the time
from a key press to the next image on screen (median of 24 presses).
Lower is better.

| Image | Metric | jog | feh | mpv | VLC |
| --- | --- | --- | --- | --- | --- |
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

```sh
jog --benchmark PHOTOS/
```

The benchmark opens the folder and steps through it, first at a normal
viewing pace and then faster than the files can decode. It then goes
back, zooms, pages through the grid and starts a video if there is one.
Each step is timed, and the results are printed before it quits.
`--debug` prints every decode, frame and wait as it happens.
