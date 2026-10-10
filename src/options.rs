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
  -Y, --hide-pointer         hide the mouse pointer

Zoom:
  -., --scale-down           shrink large images to fit (the default)
  -Z, --auto-zoom            also enlarge small images to fit (the default)
      --no-auto-zoom         show small images at their actual size
      --zoom MODE            fit, fill, or a percentage such as 100
      --keep-zoom-vp         keep zoom and position when changing image
      --force-aliasing       disable smooth scaling

File list:
  -r, --recursive            descend into directories
  -S, --sort ORDER           name, filename, dirname, mtime or size
  -n, --reverse              reverse the order
  -z, --randomize            shuffle the list
      --start-at FILE        start at this file
  -f, --filelist FILE        read paths from FILE, one per line
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
  -o, --output-dir DIR       folder that v copies and m moves images into
      --print-marked         print the marked files on exit
      --verbose              print each file as it comes up: its place in the
                             list, dimensions, size and how long it took
      --debug                print decode and video start times (also JOG_DEBUG=1)
      --no-resume            start videos from the beginning, and do not
                             remember where they were left (--resume undoes it)

Overlays:
  -d, --draw-info            show name, position, dimensions, zoom and size
      --draw-exif            show EXIF data
      --draw-actions         show the defined actions
  -t, --filmstrip            keep the filmstrip open
      --no-grid-titles       leave the names out from under the grid's pictures
                             (config: grid-titles = false; i in the grid toggles)

Config file:
  ~/.config/jog/config.toml sets defaults, using the long option names:
      output-dir = \"~/Pictures/picks\"
      draw-info = true
      auto-zoom = false
  Options on the command line override it.

  -h, --help                 show this help
  -v, --version              show the version

Keys:
  > . n Right Space          next image          < , Left Backspace  previous
  Home / End                 first / last        PgDn / PgUp   jump 5%
  z                          shuffle the list    { / }         previous / next directory
  + Up / - Down              zoom in / out       * 0 !         100%, fit, fill
  / or Ctrl+K                find: type part of a name, Up / Down to pick among
                             the matches, Enter to go to the one picked, Tab to
                             show only the matches, Esc to cancel. Right moves
                             on to the buttons: Show only the matches, or Show
                             all once a search has been kept (Esc later, too).
                             ^foo starts with, !foo without; also is:video
                             is:image is:gif is:marked, and sizes such as
                             >10mb or <1gb (b, kb, mb, gb)
  Ctrl+arrows, Alt+arrows    scroll, by a page   Z  small images: fit screen / actual size
  S                          sort: name, newest, size, shuffle, reverse
  ( / )                      rotate              _ |           flip, mirror
  [ / ]                      slower / faster     \\             normal speed
  f fullscreen  t filmstrip  i info  e EXIF  a actions  o pointer
  p                          file list down the side; click a name to go to it
  d                          delete (asks first; marked images, or the current one)
  g                          grid view           ?             list of shortcuts
  Tab or right click         menu                h             slideshow
  x                          mark / unmark       c / C         copy file name / full path
  v / m                      copy / move marked images (or the current one) to -o DIR
  u                          undo copy, move, rename or convert
  F2                         rename              r F5          refresh: look for
                             new and deleted files, and read this one again
  W                          watch: refresh by itself as files come and go
  s save copy  L save file list (marked ones if any)  w size window to image
  Delete                     remove from list    Ctrl+Delete   move to trash
  Enter, 1-9                 run action          q Esc         quit

Videos (mp4, mkv, webm and so on; needs libmpv) play in place, picking up
where they were left unless --no-resume is given. While one is up:
  Space                      play / pause        Left / Right  seek 5 seconds (faster held)
  PgUp / PgDn                seek 1 minute       pointer at the bottom: seek bar with previews
  Shift+Left / Shift+Right   step one frame      Up / Down     volume
  b                          back to the start of the video
  Ctrl+Left / Ctrl+Right, or , . < > n           previous / next file

Mouse: drag to pan, middle-drag to zoom, wheel for next/previous,
Ctrl+wheel to zoom, double click for fullscreen, bottom edge for the
filmstrip. Ctrl+Left / Ctrl+Right also change file when an image has
nothing to scroll sideways.";

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
    pub hide_pointer: bool,
    pub zoom: Zoom,
    pub auto_zoom: bool,
    pub keep_view: bool,
    pub antialias: bool,
    pub slideshow_delay: Option<f64>,
    pub on_last_slide: LastSlide,
    /// Whether the next file comes up when a video ends.
    pub autoplay: bool,
    /// Whether the grid shows each file's title under its picture.
    pub grid_titles: bool,
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
    /// Whether videos pick up where they were left.
    pub resume: bool,
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
        "geometry", "image-bg", "zoom", "sort", "start-at", "filelist", "slideshow-delay", "on-last-slide",
        "output-dir",
    ];
    valued.contains(&name) || name.starts_with("action")
}

fn is_switch(name: &str) -> bool {
    let switches = [
        "fullscreen", "windowed", "borderless", "hide-pointer", "scale-down", "version-sort", "quiet",
        "auto-zoom", "no-auto-zoom", "keep-zoom-vp", "force-aliasing", "recursive", "reverse", "randomize",
        "cycle-once", "autoplay", "no-autoplay", "grid-titles", "no-grid-titles", "draw-filename", "draw-info", "draw-exif", "draw-actions", "filmstrip", "print-marked",
        "debug", "verbose", "watch", "resume", "no-resume",
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
        hide_pointer: false,
        zoom: Zoom::Fit,
        auto_zoom: true,
        keep_view: false,
        antialias: true,
        slideshow_delay: None,
        on_last_slide: LastSlide::Resume,
        autoplay: true,
        grid_titles: true,
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
        resume: true,
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
        "resume" => options.resume = true,
        "no-resume" => options.resume = false,
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
fn config_path() -> Option<PathBuf> {
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
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

/// Applies a config file. Its keys are the long option names: switches take
/// `true` or `false`, everything else a string or a number.
fn apply_config(text: &str, options: &mut Options, fullscreen: &mut Option<bool>) -> Result<(), String> {
    let table: toml::Table = text.parse().map_err(|error: toml::de::Error| error.to_string())?;
    for (key, value) in &table {
        let key = key.replace('_', "-");
        let (name, argument) = match value {
            toml::Value::Boolean(true) => (key.as_str(), None),
            // Only the switches that default to on have an "off" spelling.
            toml::Value::Boolean(false) => match key.as_str() {
                "auto-zoom" => ("no-auto-zoom", None),
                "autoplay" => ("no-autoplay", None),
                "grid-titles" => ("no-grid-titles", None),
                "fullscreen" => ("windowed", None),
                "resume" => ("no-resume", None),
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
fn expand_home(text: &str) -> PathBuf {
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
        "mtime" => Sort::Mtime,
        "size" => Sort::Size,
        other => return Err(format!("invalid sort order '{other}'")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Options, String> {
        parse_config("", args)
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
        let options = parse_args(&["-FZ", "--scale-down", "-D2.5", "-Smtime", "a.jpg", "dir"]).unwrap();
        assert!(options.fullscreen && options.auto_zoom);
        assert!(!parse_args(&["--no-auto-zoom"]).unwrap().auto_zoom);
        assert_eq!(options.slideshow_delay, Some(2.5));
        assert_eq!(options.listing.sort, Some(Sort::Mtime));
        assert_eq!(options.paths, [PathBuf::from("a.jpg"), PathBuf::from("dir")]);

        let options = parse_args(&["-o", "/keep", "--print-marked"]).unwrap();
        assert_eq!(options.output_dir, Some(PathBuf::from("/keep")));
        assert!(options.print_marked);
        assert!(parse_args(&["--debug"]).unwrap().debug);
        assert!(parse_args(&["--verbose"]).unwrap().verbose);
        assert!(parse_args(&[]).unwrap().resume);
        assert!(!parse_args(&["--no-resume"]).unwrap().resume);
        assert!(!parse_config("resume = false", &[]).unwrap().resume);
        assert!(parse_config("resume = false", &["--resume"]).unwrap().resume);
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
