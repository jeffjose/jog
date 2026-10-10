use std::ffi::OsString;
use std::path::PathBuf;

use crate::files::{Listing, Sort};
use crate::view::Zoom;

pub const USAGE: &str = "\
Usage: jog [OPTIONS] [FILE|DIR]...

With one file, opens its directory starting at that file.
With no arguments, opens the current directory.

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

Slideshow and actions:
  -D, --slideshow-delay SEC  advance automatically
      --on-last-slide WHAT   resume (default), hold or quit
      --cycle-once           same as --on-last-slide quit
  -A, --action CMD           shell command for Enter / 0; %f is the path,
                             %n the file name
      --action1..9 CMD       shell commands for keys 1 to 9
  -o, --output-dir DIR       folder that v copies and m moves images into
      --print-marked         print the marked files on exit

Overlays:
  -d, --draw-filename        show the file name
      --draw-info            show size and zoom
      --draw-exif            show EXIF data
      --draw-actions         show the defined actions
  -t, --filmstrip            keep the filmstrip open

  -h, --help                 show this help
  -v, --version              show the version

Keys:
  > . n Right Space          next image          < , p Left Backspace  previous
  Home / End                 first / last        PgDn / PgUp   jump 5%
  z                          random image        { / }         previous / next directory
  + Up / - Down              zoom in / out       * / !         100%, fit, fill
  Ctrl+arrows, Alt+arrows    scroll, by a page   Z  enlarge small images
  [ / ]                      rotate              _ |           flip, mirror
  f fullscreen  t filmstrip  d filename  i info  e EXIF  a actions  o pointer
  g                          grid view           ?             list of shortcuts
  Tab or right click         menu                h             slideshow
  x                          mark / unmark       c / C         copy file name / full path
  v / m                      copy / move marked images (or the current one) to -o DIR
  u                          undo copy, move, rename or convert
  r F2                       rename              F5            reload
  s save copy  L save file list (marked ones if any)  w size window to image
  Delete                     remove from list    Ctrl+Delete   move to trash
  Enter, 0-9                 run action          q Esc         quit

Mouse: drag to pan, middle-drag to zoom, wheel for next/previous,
Ctrl+wheel to zoom, bottom edge for the filmstrip.";

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
    pub actions: [Option<String>; 10],
    pub draw_filename: bool,
    pub draw_info: bool,
    pub draw_exif: bool,
    pub draw_actions: bool,
    pub filmstrip: bool,
    pub output_dir: Option<PathBuf>,
    pub print_marked: bool,
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
        'd' => "draw-filename",
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

pub fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Parsed, String> {
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
        actions: Default::default(),
        draw_filename: false,
        draw_info: false,
        draw_exif: false,
        draw_actions: false,
        filmstrip: false,
        output_dir: None,
        print_marked: false,
    };
    let mut fullscreen = None;
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
                "fullscreen" => fullscreen = Some(true),
                "windowed" => fullscreen = Some(false),
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
                        other => return Err(format!("invalid --on-last-slide '{other}'")),
                    }
                }
                "cycle-once" => options.on_last_slide = LastSlide::Quit,
                "draw-filename" => options.draw_filename = true,
                "draw-info" => options.draw_info = true,
                "draw-exif" => options.draw_exif = true,
                "draw-actions" => options.draw_actions = true,
                "filmstrip" => options.filmstrip = true,
                "output-dir" => options.output_dir = Some(value()?.into()),
                "print-marked" => options.print_marked = true,
                "action" => options.actions[0] = Some(value()?),
                other => {
                    let slot = other.strip_prefix("action").and_then(|n| n.parse::<usize>().ok());
                    match slot {
                        Some(slot @ 1..=9) => options.actions[slot] = Some(value()?),
                        _ => return Err(format!("unknown option --{other}")),
                    }
                }
            }
        }
    }

    options.fullscreen = fullscreen.unwrap_or(options.geometry.is_none());
    Ok(Parsed::Run(Box::new(options)))
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
        match parse(args.iter().map(OsString::from))? {
            Parsed::Run(options) => Ok(*options),
            _ => Err("did not run".into()),
        }
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
