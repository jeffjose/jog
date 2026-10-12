use std::ffi::OsString;
use std::path::PathBuf;

use crate::files::{Listing, Sort};
use crate::view::Zoom;

pub const USAGE: &str = "\
Usage: jog [OPTIONS] [FILE|DIR]...

Shows the files named, and the images and videos in any directories named.
With no arguments, opens the current directory. To open a directory at a
particular file: jog DIR --start-at FILE

Window:
  -F, --fullscreen           start fullscreen (the default)
  -w, --windowed             start in a window
  -g, --geometry WxH         start in a window of this size
  -x, --borderless           no window decorations
  -B, --image-bg COLOR       background: black, white or RRGGBB
      --accent COLOR         colour of selections and highlights: RRGGBB, or one of
                             the Radix colours such as indigo (the default),
                             blue, violet, crimson, orange, grass or teal
  -Y, --hide-pointer         hide the mouse pointer
      --no-animations        nothing slides, fades or glides: it is simply there
                             (config: animations = false)

Zoom:
  -., --scale-down           shrink large images to fit (the default)
  -Z, --auto-zoom            also enlarge small images to fit (the default)
      --no-auto-zoom         show small images at their actual size
      --zoom MODE            fit, fill, or a percentage such as 100
      --keep-zoom-vp         keep zoom and position when changing image
      --force-aliasing       disable smooth scaling

File list:
  -r, --recursive            descend into directories
  -S, --sort ORDER           name, filename, dirname, newest, oldest,
                             largest or smallest
  -n, --reverse              reverse the order
  -z, --randomize            shuffle the list
      --start-at FILE        start at this file
  -f, --filelist FILE        read paths from FILE, one per line
      --start HOW            what coming back to this folder from another
                             workspace does: resume (the usual), fresh or
                             random, which also shuffles it now
      --filter TEXT          start with only the files the find box would show
                             for TEXT: jog --filter 'beach #video >10mb'
      --watch                add files as they appear in the directories, and
                             drop those that go (W turns it on and off)

Slideshow and actions:
  -D, --slideshow-delay SEC  advance automatically
      --on-last-slide WHAT   resume (default), hold or quit
      --cycle-once           same as --on-last-slide quit
      --no-autoplay          stay on a video when it ends, instead of
                             playing the next file (config: autoplay = false)
  -A, --action CMD           shell command for Enter / 0; %f is the path,
                             %n the file name
      --action1..9 CMD       shell commands for keys 1 to 9
  -o, --output-dir DIR       folder that V copies and M moves files into; without
                             one they ask for a folder, as Move to... in the
                             menu always does
      --print-marked         print the marked files on exit
      --dry-run              change nothing: copying, moving, renaming, deleting,
                             converting and running a command only print what
                             they would have done
      --verbose              print each file as it comes up: its place in the
                             list, dimensions, size and how long it took
      --debug                print decode and video start times (also JOG_DEBUG=1)
      --no-positions         start videos from the beginning, and do not
                             remember where they were left (config:
                             positions = false)
      --resume               pick up a session: what jog was showing when it was
                             last quit, of those started in this folder. With
                             several to choose from they are shown, to be
                             picked with the arrows or a click
      --latest               with --resume, the latest of them, unasked
      --resume ID            the session with that id, whichever folder it is
                             of; the start of an id will do when it is enough
                             to tell it by
      --sessions             list this folder's sessions here, each with its id,
                             and quit

      --precise-seeking      seek to the exact moment asked for. The usual is
                             the nearest keyframe, which is much quicker and
                             may be a few seconds off (config: precise-seeking
                             = true; Precise seeking in the menu toggles)
      --no-seen              do not remember which videos have been watched, or
                             mark the rest as new (config: seen = false)

Subtitles:
      --no-subtitles         start with subtitles hidden (j shows them)
      --subtitle-font NAME   an installed font, such as Inter (the default)
      --subtitle-size N      their size as a percentage: 100 is the usual
      --subtitle-style HOW   outline (the default): letters edged in black;
                             shadow: a soft shadow and no edge;
                             box: on a dark panel

Overlays:
  -d, --draw-info            show name, position, dimensions, zoom and size
      --draw-exif            show EXIF data
      --draw-actions         show the defined actions
  -t, --filmstrip            keep the filmstrip open
      --list-right           the file list (p) stands at the right of the window
      --list-pinned          and beside the picture, which is made smaller for
                             it, rather than over it (its pin does the same)
  -G, --grid                 start in the grid, with nothing playing
      --no-grid-titles       leave the names out from under the grid's pictures
                             (config: grid-titles = false; i in the grid toggles)

Config file:
  ~/.config/jog/config.toml sets defaults, using the long option names:
      output-dir = \"~/Pictures/picks\"
      draw-info = true
      auto-zoom = false
  Options on the command line override it.

Workspaces are folders with names, set in the config file:
      [workspaces]
      pictures = \"~/Downloads/Pictures\"
      clips = { path = \"~/Videos/clips\", recursive = true, sort = \"newest\" }
  jog @pictures starts in one. Ctrl+1 to Ctrl+9 go to them in the order
  they are written, Ctrl+Tab to the next, Ctrl+` back to what jog was
  started on, G shows them all, and @ in the find box lists
  them; there too a folder opened by its path can be saved as one.
  What going to one does is its start: \"fresh\" (the usual) lists it
  again and begins at its first file, which with sort = \"newest\" is the
  newest; \"random\" lists it again and shuffles it; \"resume\" picks
  up where it was left. Marks are kept. --start says the same of what is
  opened by its path, which otherwise resumes. In the box for
  a folder to copy or move into, @pictures means that workspace.

  -h, --help                 show this help
  -v, --version              show the version

Keys:
  > . n Right Space          next image          < , Left Backspace  previous
  Home / End                 first / last        PgDn / PgUp   jump 5%
  z                          shuffle the rest    { / }         previous / next directory
  !                          run a shell command on this file, or on the marked
                             ones: %f its path, %n its name, %s its name without
                             the ending, %d its folder, %t the time in a video.
                             Up / Down pick one run before; . runs the last again
  + Up / - Down              zoom in / out       * 0 F         100%, fit, fill
  / or Ctrl+K                find: type part of a name, Up / Down to pick among
                             the matches, Enter to go to the one picked, Esc
                             to cancel. Tab or Right moves on to the buttons:
                             Show only the matches, or Show all once a search
                             has been kept (Esc later, too).
                             ^foo starts with, !foo without; also is:video
                             is:image is:gif is:marked is:seen is:new, and sizes such as
                             >10mb or <1gb (b, kb, mb, gb)
  Ctrl+arrows, Alt+arrows    scroll, by a page   Z  small images: fit screen / actual size
  s                          sort: name, newest, size, shuffle, reverse
  ( / )                      rotate              _ |           flip, mirror
  [ / ]                      slower / faster     \\             normal speed
  f fullscreen  t filmstrip  i info  e EXIF  a actions  o pointer
  v held                     a loupe under the pointer: the wheel makes it stronger
  p                          file list down the side; click a name to go to it
                             with it up, / or its magnifying glass searches
                             in it: the list narrows as you type
  D                          delete (asks first; marked images, or the current one)
  g                          grid view           ?             list of shortcuts
                             in the grid: Ctrl with + or - for larger or smaller
                             pictures; going past the end of a page slides the
                             next one in from the side
  Tab or right click         menu                h             slideshow
  x                          mark / unmark       c / C         copy file name / full path
  V / M                      copy / move marked files (or the current one) to -o DIR
  u                          undo copy, move, rename or convert
  F2                         rename              r F5          refresh: look for
                             new and deleted files, and read this one again
  W                          watch: refresh by itself as files come and go
  S save copy  L save file list (marked ones if any)  w size window to image
  Delete                     remove from list    Ctrl+Delete   move to trash
  Enter, 1-9                 run action          q Esc         quit

Videos (mp4, mkv, webm and so on; needs libmpv) play in place, picking up
where they were left unless --no-positions is given. While one is up:
  Space                      play / pause        Left / Right  seek about 5 seconds
  PgUp / PgDn                seek about 1 minute pointer at the bottom: seek bar with previews
  Shift+Left / Shift+Right   step one frame      Up / Down     volume (m mutes)
  b                          back to the start of the video
  Space or the mouse button held down: twice the speed, until let go
  l                          loop: this video, then the whole list, then off
  T                          scenes of this video along the bottom: Left / Right
                             then go from scene to scene, and a click plays one
  j / J                      next subtitles, or none / next audio track
  Ctrl+Left / Ctrl+Right, or , . < > n           previous / next file
  Seeking past the end of a video starts the next file, and seeking back
  past its start carries on into the one before, as though they lay end to end.

Mouse: drag to pan, middle-drag to zoom, wheel for next/previous,
Ctrl+wheel to zoom, double click for fullscreen, bottom edge for the
filmstrip. Ctrl+Left / Ctrl+Right also change file when an image has
nothing to scroll sideways.";

/// How subtitles are set off from the picture behind them.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SubtitleStyle {
    /// Letters edged in black.
    Outline,
    /// A soft shadow under the letters and no edge.
    Shadow,
    /// A dark panel behind each line.
    Box,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LastSlide {
    Resume,
    Hold,
    Quit,
}

pub struct Options {
    pub paths: Vec<PathBuf>,
    pub listing: Listing,
    pub fullscreen: bool,
    pub geometry: Option<(u32, u32)>,
    pub borderless: bool,
    pub background: u32,
    /// The colour of what is picked out: selections, the grid's cursor.
    pub accent: u32,
    pub hide_pointer: bool,
    pub zoom: Zoom,
    pub auto_zoom: bool,
    pub keep_view: bool,
    pub antialias: bool,
    pub slideshow_delay: Option<f64>,
    pub on_last_slide: LastSlide,
    /// Whether the next file comes up when a video ends.
    pub autoplay: bool,
    /// Whether the file list stands at the right, and beside the picture
    /// rather than over it.
    pub list_right: bool,
    pub list_pinned: bool,
    /// Whether what would change a file or run a command is only said.
    pub dry_run: bool,
    /// Whether seeking lands on the exact moment or the nearest keyframe.
    pub precise_seeking: bool,
    /// Whether things move into place or are simply there.
    pub animations: bool,
    /// Whether the videos watched are remembered and the rest marked new.
    pub seen: bool,
    /// Whether a video's subtitles are shown, and how they are drawn.
    pub subtitles: bool,
    pub subtitle_font: String,
    /// Their size, as a percentage of the usual.
    pub subtitle_size: u32,
    pub subtitle_style: SubtitleStyle,
    /// Whether the grid shows each file's title under its picture.
    pub grid_titles: bool,
    /// Whether to start in the grid.
    pub grid: bool,
    pub actions: [Option<String>; 10],
    pub draw_info: bool,
    pub draw_exif: bool,
    pub draw_actions: bool,
    pub filmstrip: bool,
    pub output_dir: Option<PathBuf>,
    pub print_marked: bool,
    pub debug: bool,
    pub verbose: bool,
    pub watch: bool,
    /// What to narrow the list to from the start, as typed in the find box.
    pub filter: Option<String>,
    /// What coming back to what was opened does, if it was said.
    pub start: Option<Start>,
    /// The named folders of the config file, in its order, and which of
    /// them, if any, was asked for by name to start in.
    pub workspaces: Vec<Workspace>,
    pub workspace: Option<usize>,
    /// Whether videos pick up where they were left.
    pub resume: bool,
    /// Whether to pick up a session of this folder, the latest of them
    /// unasked, or only to list them; and, set by `main` once it has
    /// looked, the ones there are to choose from and how far into its
    /// video the one picked up was.
    pub session: bool,
    pub latest: bool,
    pub list_sessions: bool,
    pub sessions: Vec<crate::sessions::Session>,
    pub begin_at: Option<f64>,
    /// The id of the session picked up, which it keeps when kept again.
    pub session_id: Option<String>,
}

/// A folder with a name, to switch to by it. It may say how it is to be
/// listed; left unsaid, that is as the options have it.
#[derive(Clone, Debug, PartialEq)]
pub struct Workspace {
    pub name: String,
    pub path: PathBuf,
    pub recursive: Option<bool>,
    pub sort: Option<Sort>,
    /// That order backwards.
    pub reverse: bool,
    /// The digit that goes to it with Ctrl: the one it asked for, or the
    /// first that no other has taken. None when the nine are used up.
    pub key: Option<usize>,
    /// What going to it does.
    pub start: Start,
}

/// What going to a workspace does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Start {
    /// List it again and begin at its first file: with a sort by
    /// newest first, the newest.
    Fresh,
    /// List it again and shuffle it.
    Random,
    /// Leave it as it was, and pick up where it was left.
    Resume,
}

fn parse_start(text: &str) -> Result<Start, String> {
    Ok(match text {
        "fresh" => Start::Fresh,
        "random" => Start::Random,
        "resume" => Start::Resume,
        _ => return Err(format!("invalid start '{text}', expected fresh, random or resume")),
    })
}

/// Reads the `[workspaces]` table: each entry a path, or a table of a
/// path and how to list it.
fn parse_workspaces(value: &toml::Value) -> Result<Vec<Workspace>, String> {
    let table = value.as_table().ok_or("workspaces: expected a table of names and folders")?;
    let mut workspaces = Vec::new();
    for (name, entry) in table {
        let plain = |path: &str| Workspace { name: name.clone(), path: expand_home(path), recursive: None, sort: None, reverse: false, start: Start::Fresh, key: None };
        let workspace = match entry {
            toml::Value::String(path) => plain(path),
            toml::Value::Table(details) => {
                // A misspelt setting is better refused than left unheeded.
                if let Some(unknown) = details.keys().find(|key| !["path", "recursive", "sort", "reverse", "start", "key"].contains(&key.as_str())) {
                    return Err(format!("workspace {name}: unknown setting {unknown} (there are path, recursive, sort, reverse, start and key)"));
                }
                let path = details.get("path").and_then(toml::Value::as_str);
                let mut workspace = plain(path.ok_or_else(|| format!("workspace {name}: needs a path"))?);
                workspace.recursive = details.get("recursive").and_then(toml::Value::as_bool);
                workspace.reverse = details.get("reverse").and_then(toml::Value::as_bool).unwrap_or(false);
                if let Some(start) = details.get("start").and_then(toml::Value::as_str) {
                    workspace.start = parse_start(start)?;
                }
                if let Some(key) = details.get("key") {
                    let digit = key.as_integer().filter(|digit| (1..=9).contains(digit));
                    workspace.key = Some(digit.ok_or_else(|| format!("workspace {name}: key is a digit from 1 to 9"))? as usize);
                }
                if let Some(order) = details.get("sort").and_then(toml::Value::as_str) {
                    workspace.sort = Some(parse_sort(order)?);
                }
                workspace
            }
            _ => return Err(format!("workspace {name}: expected a folder, or a table with a path")),
        };
        workspaces.push(workspace);
    }
    // Those that ask for a digit have it, no two the same; the rest take
    // the digits left over, lowest first, in the order they are written.
    let mut taken: Vec<usize> = Vec::new();
    for workspace in &workspaces {
        if let Some(key) = workspace.key {
            if taken.contains(&key) {
                return Err(format!("workspaces: more than one asks for key {key}"));
            }
            taken.push(key);
        }
    }
    for workspace in workspaces.iter_mut().filter(|workspace| workspace.key.is_none()) {
        workspace.key = (1..=9).find(|key| !taken.contains(key));
        taken.extend(workspace.key);
    }
    Ok(workspaces)
}

/// Adds a workspace to the config file, which is made if it is not there.
/// The line goes into the `[workspaces]` table, itself added at the end
/// if need be; nothing else in the file is touched. Returns the file.
pub fn save_workspace(name: &str, path: &std::path::Path) -> Result<PathBuf, String> {
    let file = config_path().ok_or("there is no home directory to keep a config file in")?;
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("cannot read {}: {error}", file.display())),
    };
    let quoted = |text: &str| format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""));
    // A name of plain letters needs no quotes, as a key.
    let bare = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    let line = format!("{} = {}", if bare { name.to_string() } else { quoted(name) }, quoted(&path.to_string_lossy()));
    let mut lines: Vec<&str> = text.lines().collect();
    match lines.iter().position(|existing| existing.trim() == "[workspaces]") {
        // After the last entry of the table: before the next table, or
        // the blank lines leading up to it.
        Some(header) => {
            let next = lines[header + 1..].iter().position(|existing| existing.trim_start().starts_with('['));
            let mut at = next.map_or(lines.len(), |next| header + 1 + next);
            while at > header + 1 && lines[at - 1].trim().is_empty() {
                at -= 1;
            }
            lines.insert(at, &line);
        }
        None => {
            if lines.last().is_some_and(|last| !last.trim().is_empty()) {
                lines.push("");
            }
            lines.extend(["[workspaces]", &line]);
        }
    }
    if let Some(folder) = file.parent() {
        std::fs::create_dir_all(folder).map_err(|error| format!("cannot make {}: {error}", folder.display()))?;
    }
    std::fs::write(&file, lines.join("\n") + "\n").map_err(|error| format!("cannot write {}: {error}", file.display()))?;
    Ok(file)
}

pub enum Parsed {
    Run(Box<Options>),
    Help,
    Version,
}

/// The long name of each short flag.
fn short_flag(flag: char) -> Option<&'static str> {
    Some(match flag {
        'F' => "fullscreen",
        'G' => "grid",
        'w' => "windowed",
        'g' => "geometry",
        'x' => "borderless",
        'B' => "image-bg",
        'Y' => "hide-pointer",
        '.' => "scale-down",
        'Z' => "auto-zoom",
        'r' => "recursive",
        'S' => "sort",
        'n' => "reverse",
        'z' => "randomize",
        'V' => "version-sort",
        'f' => "filelist",
        'D' => "slideshow-delay",
        'A' => "action",
        'd' => "draw-info",
        't' => "filmstrip",
        'o' => "output-dir",
        'q' => "quiet",
        'h' => "help",
        'v' => "version",
        _ => return None,
    })
}

fn takes_value(name: &str) -> bool {
    let valued = [
        "geometry", "image-bg", "accent", "zoom", "sort", "start-at", "start", "filelist", "filter", "slideshow-delay", "on-last-slide",
        "output-dir", "subtitle-font", "subtitle-size", "subtitle-style",
    ];
    valued.contains(&name) || name.starts_with("action")
}

fn is_switch(name: &str) -> bool {
    let switches = [
        "fullscreen", "windowed", "borderless", "hide-pointer", "scale-down", "version-sort", "quiet",
        "auto-zoom", "no-auto-zoom", "keep-zoom-vp", "force-aliasing", "recursive", "reverse", "randomize",
        "cycle-once", "autoplay", "no-autoplay", "grid", "grid-titles", "no-grid-titles", "draw-filename", "draw-info", "draw-exif", "draw-actions", "filmstrip", "print-marked",
        "debug", "verbose", "watch", "positions", "no-positions", "resume", "latest", "sessions", "subtitles", "no-subtitles", "seen", "no-seen", "animations", "no-animations", "precise-seeking", "no-precise-seeking", "dry-run", "list-right", "list-pinned",
    ];
    switches.contains(&name)
}

/// Builds the options from the config file, then the command line, which
/// wins where both set something.
pub fn parse(args: impl Iterator<Item = OsString>) -> Result<Parsed, String> {
    parse_with(args, read_config()?)
}

fn parse_with(
    mut args: impl Iterator<Item = OsString>,
    config: Option<(PathBuf, String)>,
) -> Result<Parsed, String> {
    let mut options = Options {
        paths: Vec::new(),
        listing: Listing::default(),
        fullscreen: true,
        geometry: None,
        borderless: false,
        background: 0x000000,
        accent: 0x3E63DD,
        hide_pointer: false,
        zoom: Zoom::Fit,
        auto_zoom: true,
        keep_view: false,
        antialias: true,
        slideshow_delay: None,
        on_last_slide: LastSlide::Resume,
        autoplay: true,
        list_right: false,
        list_pinned: false,
        dry_run: false,
        precise_seeking: false,
        animations: true,
        seen: true,
        subtitles: true,
        subtitle_font: "Inter".to_string(),
        subtitle_size: 100,
        subtitle_style: SubtitleStyle::Outline,
        grid_titles: true,
        grid: false,
        actions: Default::default(),
        draw_info: false,
        draw_exif: false,
        draw_actions: false,
        filmstrip: false,
        output_dir: None,
        print_marked: false,
        debug: std::env::var_os("JOG_DEBUG").is_some(),
        verbose: false,
        watch: false,
        filter: None,
        start: None,
        workspaces: Vec::new(),
        workspace: None,
        resume: true,
        session: false,
        latest: false,
        list_sessions: false,
        sessions: Vec::new(),
        begin_at: None,
        session_id: None,
    };
    let mut fullscreen = None;
    if let Some((path, text)) = config {
        apply_config(&text, &mut options, &mut fullscreen).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    let mut only_paths = false;

    while let Some(arg) = args.next() {
        let text = match arg.to_str() {
            Some(text) if !only_paths && text.len() > 1 && text.starts_with('-') => text,
            _ => {
                options.paths.push(arg.into());
                continue;
            }
        };
        if text == "--" {
            only_paths = true;
            continue;
        }

        // Each entry is a flag name and any value attached to it.
        let mut flags: Vec<(String, Option<String>)> = Vec::new();
        if let Some(long) = text.strip_prefix("--") {
            match long.split_once('=') {
                Some((name, value)) => flags.push((name.into(), Some(value.into()))),
                None => flags.push((long.into(), None)),
            }
        } else {
            let cluster = &text[1..];
            for (at, flag) in cluster.char_indices() {
                let name = short_flag(flag).ok_or_else(|| format!("unknown option -{flag}"))?;
                let rest = &cluster[at + flag.len_utf8()..];
                if takes_value(name) && !rest.is_empty() {
                    flags.push((name.into(), Some(rest.into())));
                    break;
                }
                flags.push((name.into(), None));
            }
        }

        for (name, attached) in flags {
            let mut value = || -> Result<String, String> {
                let next = attached.clone().or_else(|| args.next().and_then(|a| a.into_string().ok()));
                next.ok_or_else(|| format!("option --{name} needs a value"))
            };
            match name.as_str() {
                "help" => return Ok(Parsed::Help),
                "version" => return Ok(Parsed::Version),
                name => apply(&mut options, &mut fullscreen, name, &mut value).map_err(|error| {
                    // `apply` names options the way the config file spells them.
                    error.replace("unknown option ", "unknown option --")
                })?,
            }
        }
    }

    options.fullscreen = fullscreen.unwrap_or(options.geometry.is_none());
    // `@name` stands for the workspace of that name, and starts in it.
    for at in 0..options.paths.len() {
        let Some(name) = options.paths[at].to_str().and_then(|text| text.strip_prefix('@')).map(str::to_string) else { continue };
        let Some(found) = options.workspaces.iter().position(|space| space.name == name) else {
            let known: Vec<&str> = options.workspaces.iter().map(|space| space.name.as_str()).collect();
            let known = if known.is_empty() { "none are set in the config file".to_string() } else { format!("there are: {}", known.join(", ")) };
            return Err(format!("no workspace named '{name}' ({known})"));
        };
        options.paths[at] = options.workspaces[found].path.clone();
        options.workspace = Some(found);
    }
    // One started in brings its own way of listing.
    if let Some(space) = options.workspace.map(|found| options.workspaces[found].clone()) {
        options.listing.recursive |= space.recursive.unwrap_or(false);
        options.listing.sort = options.listing.sort.or(space.sort);
        options.listing.reverse |= space.reverse;
        options.start = options.start.or(Some(space.start));
    }
    // One started at random is shuffled from the first.
    if options.start == Some(Start::Random) {
        options.listing.randomize = true;
    }
    Ok(Parsed::Run(Box::new(options)))
}

/// Applies one option by its long name, whether it came from the command
/// line or the config file. `value` fetches the option's argument.
fn apply(
    options: &mut Options,
    fullscreen: &mut Option<bool>,
    name: &str,
    value: &mut dyn FnMut() -> Result<String, String>,
) -> Result<(), String> {
    match name {
        "fullscreen" => *fullscreen = Some(true),
        "windowed" => *fullscreen = Some(false),
        "geometry" => options.geometry = Some(parse_geometry(&value()?)?),
        "borderless" => options.borderless = true,
        "image-bg" => options.background = parse_color(&value()?)?,
        "accent" => options.accent = parse_accent(&value()?)?,
        "hide-pointer" => options.hide_pointer = true,
        // Fitting large images and natural sorting are always on.
        "scale-down" | "version-sort" | "quiet" => {}
        "auto-zoom" => options.auto_zoom = true,
        "no-auto-zoom" => options.auto_zoom = false,
        "zoom" => options.zoom = parse_zoom(&value()?)?,
        "keep-zoom-vp" => options.keep_view = true,
        "force-aliasing" => options.antialias = false,
        "recursive" => options.listing.recursive = true,
        "sort" => options.listing.sort = Some(parse_sort(&value()?)?),
        "reverse" => options.listing.reverse = true,
        "randomize" => options.listing.randomize = true,
        "start-at" => options.listing.start_at = Some(value()?.into()),
        "filter" => options.filter = Some(value()?),
        "start" => options.start = Some(parse_start(&value()?)?),
        "filelist" => options.listing.filelist = Some(value()?.into()),
        "slideshow-delay" => {
            let text = value()?;
            let seconds: f64 = text.parse().map_err(|_| format!("invalid delay '{text}'"))?;
            options.slideshow_delay = (seconds > 0.0).then_some(seconds);
        }
        "on-last-slide" => {
            options.on_last_slide = match value()?.as_str() {
                "resume" => LastSlide::Resume,
                "hold" => LastSlide::Hold,
                "quit" => LastSlide::Quit,
                other => return Err(format!("invalid on-last-slide '{other}'")),
            }
        }
        "cycle-once" => options.on_last_slide = LastSlide::Quit,
        "autoplay" => options.autoplay = true,
        "no-autoplay" => options.autoplay = false,
        "list-right" => options.list_right = true,
        "list-pinned" => options.list_pinned = true,
        "dry-run" => options.dry_run = true,
        "precise-seeking" => options.precise_seeking = true,
        "no-precise-seeking" => options.precise_seeking = false,
        "animations" => options.animations = true,
        "no-animations" => options.animations = false,
        "seen" => options.seen = true,
        "no-seen" => options.seen = false,
        "subtitles" => options.subtitles = true,
        "no-subtitles" => options.subtitles = false,
        "subtitle-font" => options.subtitle_font = value()?,
        "subtitle-size" => {
            let text = value()?;
            let size = text.trim_end_matches('%').parse::<u32>().ok().filter(|size| (25..=400).contains(size));
            options.subtitle_size = size.ok_or_else(|| format!("invalid subtitle size '{text}', expected a percentage from 25 to 400"))?;
        }
        "subtitle-style" => {
            options.subtitle_style = match value()?.as_str() {
                "outline" => SubtitleStyle::Outline,
                "shadow" => SubtitleStyle::Shadow,
                "box" => SubtitleStyle::Box,
                other => return Err(format!("invalid subtitle style '{other}', expected outline, shadow or box")),
            }
        }
        "grid" => options.grid = true,
        "grid-titles" => options.grid_titles = true,
        "no-grid-titles" => options.grid_titles = false,
        // feh's name for it; here the file name is part of the info overlay.
        "draw-filename" | "draw-info" => options.draw_info = true,
        "draw-exif" => options.draw_exif = true,
        "draw-actions" => options.draw_actions = true,
        "filmstrip" => options.filmstrip = true,
        "output-dir" => options.output_dir = Some(expand_home(&value()?)),
        "print-marked" => options.print_marked = true,
        "debug" => options.debug = true,
        "verbose" => options.verbose = true,
        "watch" => options.watch = true,
        "positions" => options.resume = true,
        "no-positions" => options.resume = false,
        "resume" => options.session = true,
        "latest" => options.latest = true,
        "sessions" => options.list_sessions = true,
        "action" => options.actions[0] = Some(value()?),
        other => {
            let slot = other.strip_prefix("action").and_then(|n| n.parse::<usize>().ok());
            match slot {
                Some(slot @ 1..=9) => options.actions[slot] = Some(value()?),
                _ => return Err(format!("unknown option {other}")),
            }
        }
    }
    Ok(())
}

/// Where the config file lives: `$JOG_CONFIG`, or `jog/config.toml` under
/// `$XDG_CONFIG_HOME` or `~/.config`.
pub fn config_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("JOG_CONFIG") {
        return Some(path.into());
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")))?;
    Some(base.join("jog/config.toml"))
}

/// The config file's path and contents, if there is one.
fn read_config() -> Result<Option<(PathBuf, String)>, String> {
    let Some(path) = config_path() else { return Ok(None) };
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some((path, text))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // The first time, leave a file to start from: every setting,
            // commented out. Only in the usual place, not one named for
            // the occasion; and a failure to write it is no failure to run.
            if std::env::var_os("JOG_CONFIG").is_none() {
                let made = path.parent().is_some_and(|folder| std::fs::create_dir_all(folder).is_ok());
                if made && std::fs::write(&path, STARTER_CONFIG).is_ok() {
                    eprintln!("jog: wrote a config file to start from: {}", path.display());
                }
            }
            Ok(None)
        }
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

/// The config file written when there is none: nothing set, everything
/// that can be set shown commented out with its default.
const STARTER_CONFIG: &str = include_str!("../assets/config.toml");

/// Applies a config file. Its keys are the long option names: switches take
/// `true` or `false`, everything else a string or a number.
fn apply_config(text: &str, options: &mut Options, fullscreen: &mut Option<bool>) -> Result<(), String> {
    let table: toml::Table = text.parse().map_err(|error: toml::de::Error| error.to_string())?;
    for (key, value) in &table {
        // The one table: named folders to switch between.
        if key == "workspaces" {
            options.workspaces = parse_workspaces(value)?;
            continue;
        }
        let key = key.replace('_', "-");
        let (name, argument) = match value {
            toml::Value::Boolean(true) => (key.as_str(), None),
            // Only the switches that default to on have an "off" spelling.
            toml::Value::Boolean(false) => match key.as_str() {
                "auto-zoom" => ("no-auto-zoom", None),
                "autoplay" => ("no-autoplay", None),
                "animations" => ("no-animations", None),
                "seen" => ("no-seen", None),
                "subtitles" => ("no-subtitles", None),
                "grid-titles" => ("no-grid-titles", None),
                "fullscreen" => ("windowed", None),
                "positions" => ("no-positions", None),
                other if is_switch(other) => continue,
                _ => (key.as_str(), None),
            },
            toml::Value::String(text) => (key.as_str(), Some(text.clone())),
            toml::Value::Integer(number) => (key.as_str(), Some(number.to_string())),
            toml::Value::Float(number) => (key.as_str(), Some(number.to_string())),
            _ => return Err(format!("{key}: expected a string, a number, true or false")),
        };
        if is_switch(name) && argument.is_some() {
            return Err(format!("{key} is a switch: use true or false"));
        }
        if takes_value(name) && argument.is_none() {
            return Err(format!("{key} needs a value"));
        }
        apply(options, fullscreen, name, &mut || Ok(argument.clone().unwrap_or_default()))?;
    }
    Ok(())
}

/// Expands a leading `~`, which the shell does not do for config values.
pub fn expand_home(text: &str) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match (text.strip_prefix('~'), home) {
        (Some(""), Some(home)) => home,
        (Some(rest), Some(home)) if rest.starts_with('/') => home.join(&rest[1..]),
        _ => PathBuf::from(text),
    }
}

fn parse_geometry(text: &str) -> Result<(u32, u32), String> {
    let size = text.split('+').next().unwrap_or(text);
    let parsed = size.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
    parsed.filter(|&(w, h)| w > 0 && h > 0).ok_or_else(|| format!("invalid geometry '{text}', expected WxH"))
}

fn parse_color(text: &str) -> Result<u32, String> {
    match text {
        "black" => Ok(0x000000),
        "white" => Ok(0xFFFFFF),
        _ => {
            let hex = text.trim_start_matches('#');
            let color = u32::from_str_radix(hex, 16).ok().filter(|_| hex.len() == 6);
            color.ok_or_else(|| format!("invalid colour '{text}', expected black, white or RRGGBB"))
        }
    }
}

/// An accent colour: one of the Radix colours by name, at the step meant
/// for solid backgrounds, or any colour as `RRGGBB`.
fn parse_accent(text: &str) -> Result<u32, String> {
    let named = match text.to_lowercase().as_str() {
        "tomato" => 0xE54D2E,
        "red" => 0xE5484D,
        "ruby" => 0xE54666,
        "crimson" => 0xE93D82,
        "pink" => 0xD6409F,
        "plum" => 0xAB4ABA,
        "purple" => 0x8E4EC6,
        "violet" => 0x6E56CF,
        "iris" => 0x5B5BD6,
        "indigo" => 0x3E63DD,
        "blue" => 0x0090FF,
        "cyan" => 0x00A2C7,
        "teal" => 0x12A594,
        "jade" => 0x29A383,
        "green" => 0x30A46C,
        "grass" => 0x46A758,
        "orange" => 0xF76B15,
        "brown" => 0xAD7F58,
        "bronze" => 0xA18072,
        "gold" => 0x978365,
        _ => return parse_color(text).map_err(|_| format!("invalid accent '{text}', expected a Radix colour name such as indigo, or RRGGBB")),
    };
    Ok(named)
}

fn parse_zoom(text: &str) -> Result<Zoom, String> {
    match text {
        "fit" | "max" => Ok(Zoom::Fit),
        "fill" => Ok(Zoom::Fill),
        _ => {
            let percent = text.trim_end_matches('%').parse::<f64>().ok().filter(|p| *p > 0.0);
            percent.map(|p| Zoom::Scale(p / 100.0)).ok_or_else(|| format!("invalid zoom '{text}'"))
        }
    }
}

fn parse_sort(text: &str) -> Result<Sort, String> {
    Ok(match text {
        "name" => Sort::Name,
        "filename" => Sort::Filename,
        "dirname" => Sort::Dirname,
        "newest" => Sort::Newest,
        "oldest" => Sort::Oldest,
        "largest" => Sort::Largest,
        "smallest" => Sort::Smallest,
        other => return Err(format!("invalid sort order '{other}', expected name, filename, dirname, newest, oldest, largest or smallest")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Options, String> {
        parse_config("", args)
    }

    #[test]
    fn the_starter_config_sets_nothing_and_every_line_of_it_can_be_used() {
        let untouched = parse_config(STARTER_CONFIG, &[]).unwrap();
        assert!(untouched.workspaces.is_empty() && untouched.output_dir.is_none() && untouched.fullscreen);

        // With the # taken off every setting, it is still a config file.
        let setting = |line: &str| {
            let rest = line.strip_prefix("# ")?;
            let named = !rest.starts_with(char::is_whitespace) && rest.split_once('=').is_some_and(|(key, _)| !key.trim().is_empty() && key.trim().chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
            (named || rest.starts_with('[')).then(|| rest.to_string())
        };
        let used: Vec<String> = STARTER_CONFIG.lines().map(|line| setting(line).unwrap_or_else(|| line.to_string())).collect();
        let options = parse_config(&used.join("\n"), &[]).unwrap();
        assert_eq!(options.workspaces.len(), 4);
        let starts: Vec<Start> = options.workspaces.iter().map(|space| space.start).collect();
        assert_eq!(starts, [Start::Fresh, Start::Fresh, Start::Random, Start::Resume]);
        assert_eq!(options.accent, 0x3E63DD);
        assert!(options.actions[1].is_some());
    }

    #[test]
    fn workspaces_are_read_in_order_and_started_in_by_name() {
        let config = "[workspaces]\nzebra = \"/z\"\nclips = { path = \"/c\", recursive = true, sort = \"newest\" }\n";
        let options = parse_config(config, &["@clips"]).unwrap();
        let names: Vec<&str> = options.workspaces.iter().map(|space| space.name.as_str()).collect();
        assert_eq!(names, ["zebra", "clips"]);
        assert_eq!(options.workspace, Some(1));
        assert_eq!(options.paths, [PathBuf::from("/c")]);
        assert!(options.listing.recursive);
        assert_eq!(options.listing.sort, Some(Sort::Newest));
        assert!(parse_config(config, &["@nope"]).is_err());
    }

    fn parse_config(config: &str, args: &[&str]) -> Result<Options, String> {
        let config = Some((PathBuf::from("config.toml"), config.to_string()));
        match parse_with(args.iter().map(OsString::from), config)? {
            Parsed::Run(options) => Ok(*options),
            _ => Err("did not run".into()),
        }
    }

    #[test]
    fn config_sets_defaults_and_the_command_line_overrides() {
        let config = "output-dir = \"/picks\"\nauto_zoom = false\nfullscreen = false\nslideshow-delay = 4\ndraw-filename = true\nrecursive = false\naction1 = \"gimp %f\"\n";
        let options = parse_config(config, &[]).unwrap();
        assert_eq!(options.output_dir, Some(PathBuf::from("/picks")));
        assert!(!options.auto_zoom && !options.fullscreen && options.draw_info && !options.listing.recursive);
        assert_eq!(options.slideshow_delay, Some(4.0));
        assert_eq!(options.actions[1].as_deref(), Some("gimp %f"));

        let options = parse_config(config, &["-o", "/other", "-FZ"]).unwrap();
        assert_eq!(options.output_dir, Some(PathBuf::from("/other")));
        assert!(options.auto_zoom && options.fullscreen);
    }

    #[test]
    fn config_mistakes_are_reported() {
        let error = |config| parse_config(config, &[]).err().unwrap();
        assert!(error("bogus = 1").contains("config.toml: unknown option bogus"));
        assert!(error("output-dir = true").contains("needs a value"));
        assert!(error("recursive = \"yes\"").contains("is a switch"));
        assert!(error("sort = \"sideways\"").contains("invalid sort order"));
        assert!(error("not toml at all").starts_with("config.toml: "));
    }

    #[test]
    fn home_is_expanded_in_folders() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        assert_eq!(expand_home("~/picks"), home.join("picks"));
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("/abs/~/x"), PathBuf::from("/abs/~/x"));
        assert_eq!(expand_home("~other/x"), PathBuf::from("~other/x"));
    }

    #[test]
    fn feh_style_invocation() {
        let options = parse_args(&["-FZ", "--scale-down", "-D2.5", "-Snewest", "a.jpg", "dir"]).unwrap();
        assert!(options.fullscreen && options.auto_zoom);
        assert!(!parse_args(&["--no-auto-zoom"]).unwrap().auto_zoom);
        assert_eq!(options.slideshow_delay, Some(2.5));
        assert_eq!(options.listing.sort, Some(Sort::Newest));
        assert_eq!(options.paths, [PathBuf::from("a.jpg"), PathBuf::from("dir")]);

        let options = parse_args(&["-o", "/keep", "--print-marked"]).unwrap();
        assert_eq!(options.output_dir, Some(PathBuf::from("/keep")));
        assert!(options.print_marked);
        assert!(parse_args(&["--debug"]).unwrap().debug);
        assert!(parse_args(&["--verbose"]).unwrap().verbose);
        assert!(parse_args(&[]).unwrap().resume);
        assert!(!parse_args(&["--no-positions"]).unwrap().resume);
        assert!(!parse_config("positions = false", &[]).unwrap().resume);
        assert!(parse_config("positions = false", &["--positions"]).unwrap().resume);
        assert!(parse_args(&[]).unwrap().autoplay);
        assert!(!parse_args(&["--no-autoplay"]).unwrap().autoplay);
        assert!(parse_args(&[]).unwrap().grid_titles);
        assert!(!parse_config("grid-titles = false", &[]).unwrap().grid_titles);
    }

    #[test]
    fn values_can_be_attached_or_separate() {
        let options = parse_args(&["--zoom=fill", "--geometry", "800x600+10+10", "--action2", "echo %f", "-A", "rm %f"]).unwrap();
        assert_eq!(options.zoom, Zoom::Fill);
        assert_eq!(options.geometry, Some((800, 600)));
        assert!(!options.fullscreen, "a geometry implies a window");
        assert_eq!(options.actions[2].as_deref(), Some("echo %f"));
        assert_eq!(options.actions[0].as_deref(), Some("rm %f"));
    }

    #[test]
    fn double_dash_ends_options() {
        let options = parse_args(&["--", "-odd-name.jpg"]).unwrap();
        assert_eq!(options.paths, [PathBuf::from("-odd-name.jpg")]);
    }

    #[test]
    fn bad_input_is_rejected() {
        assert!(parse_args(&["--nope"]).is_err());
        assert!(parse_args(&["-Q"]).is_err());
        assert!(parse_args(&["--zoom"]).is_err());
        assert!(parse_args(&["--image-bg", "checks"]).is_err());
    }
}
