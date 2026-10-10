# Ideas not built yet

Things that have been talked through and are waiting to be built. Nothing
here exists in the code.

## Follow-ups to search

- Match folder names too when listing with `-r`
- Highlight the matched letters in the grid footer
- Paste into the search and rename boxes

## Follow-ups to remembering video positions

- Show the frame being resumed to while the player starts, instead of the
  opening frame
- Remember volume, speed and rotation per file as well

## Video

- Save or copy the current frame, named with its timestamp
- In / out points, lossless clip export, loop between them
- Technical details panel on `e`: codec, frame rate, live bitrate
- Mute, playback speed, loop, audio and subtitle track switching
- Chapter marks and bookmarks on the seek bar; scene-change ticks
- Storyboard view: a grid of frames across one video
- Brightness, contrast, saturation; deinterlace; crop
- GPU rendering as an option for files too heavy for CPU decoding

## Reviewing

- Show only marked; jump to next / previous marked
- Star ratings or colour labels, with filtering
- Remember marks per directory between runs
- Name clashes in the output folder: skip identical files, number
  different ones; `{date}` in the folder path
- Compare two images side by side with linked zoom

## Viewing

- Faster JPEG loading by decoding at reduced size
- AVIF and HEIC
- User-defined menu commands and key bindings in `config.toml`
