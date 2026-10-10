# Ideas not built yet

Designs that have been talked through and are waiting to be built. Nothing
here exists in the code.

## Search and filter (`/`)

One text box, opened with `/`, that narrows the list as you type. It works
for images and videos, in the single view and in the grid.

Two ways to finish:

- **Jump:** Enter goes to the highlighted match and restores the full list.
- **Keep the filter:** a second key (Tab or Ctrl+Enter) closes the box and
  leaves the list narrowed to the matches. Next/previous, slideshow, random,
  the grid, marks and file commands then only see those files.
- Esc cancels and returns to where you were.

Matching:

- Fuzzy by default: typed letters must appear in order, not adjacent
  (`brk` finds `berserker.mp4`). Contiguous and start-of-name matches rank
  first.
- A leading `^` anchors to the start of the name, for prefix search.
- Case-insensitive, on the file name; with `-r`, on the folder part too.
- Possible extras: `.mp4` to filter by type, `!foo` to exclude.

Display:

- Grid: shows only the matches, re-flowing as you type; arrow keys still
  move the cursor through the results while the box is open.
- Single view: the filmstrip opens and shows the matches, with the top
  match previewed (a still for a video, so typing does not start playback).
- While a filter is active, a label shows it (`foo · 10 of 318`) and the
  position counter reads `[3/10]`. Esc or an empty search clears it.

Open decisions:

1. `/` is "fit to screen" today. It needs a new key (`=` or `0`; `0` is the
   action-0 key).
2. Whether Enter jumps and Tab keeps the filter, or the other way round.
3. "Mark all" under a filter would mark only the visible matches. Confirm
   that is wanted.
4. Whether a filter survives leaving the grid (suggested: yes, it belongs
   to the list, not the view).

Implementation note: the list is a plain sequence today. A filter means
keeping the full list alongside the visible one, so that clearing the
filter, undoing a move, and shuffling all still behave.

## Remember the last position in a video

Reopening a video continues from where it was left.

- On by default.
- Controllable from `config.toml` and by a command-line flag.
- Positions stored in a file under the user's config area (`~/.config/jog/`
  was suggested; `~/.local/state/jog/` is the conventional place for state
  that is not settings, so that is worth deciding).

To settle when building:

- What identifies a file: its full path, or path plus size and modified
  time so that a replaced file does not inherit an old position.
- When not to resume: within a few seconds of the start, or near the end
  (start over instead).
- A way to restart from the beginning (Home, or a menu entry).
- How many entries to keep before dropping the oldest.
- Whether going to the grid and back should use the same mechanism, which
  would fix the video restarting from the start there.

## Other ideas raised

Video:

- Save or copy the current frame, named with its timestamp
- In / out points, lossless clip export, loop between them
- Technical details panel on `e`: codec, frame rate, live bitrate
- Mute, playback speed, loop, audio and subtitle track switching
- Chapter marks and bookmarks on the seek bar; scene-change ticks
- Storyboard view: a grid of frames across one video
- Brightness, contrast, saturation; deinterlace; crop
- Play the next file automatically when a video ends
- GPU rendering as an option for files too heavy for CPU decoding

Reviewing:

- Show only marked; jump to next / previous marked
- Star ratings or colour labels, with filtering
- Remember marks per directory between runs
- Name clashes in the output folder: skip identical files, number
  different ones; `{date}` in the folder path
- Compare two images side by side with linked zoom

Viewing:

- Animated GIF and WebP playback
- Faster JPEG loading by decoding at reduced size
- AVIF and HEIC
- User-defined menu commands and key bindings in `config.toml`
- Paste in the rename box
