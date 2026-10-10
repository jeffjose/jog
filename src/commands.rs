use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::convert::Format;
use crate::files::Sort;

/// Everything the viewer can be told to do, from a key, the mouse or the menu.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Command {
    Next,
    Previous,
    First,
    Last,
    JumpForward,
    JumpBack,
    Random,
    NextDir,
    PreviousDir,
    ZoomIn,
    ZoomOut,
    ZoomActual,
    ZoomFit,
    ZoomFill,
    ToggleEnlarge,
    ToggleKeepView,
    /// Direction to move the viewport in, and whether by a whole page.
    Scroll(i8, i8, bool),
    RotateRight,
    RotateLeft,
    /// Playback speed: a step faster or slower, or back to normal.
    SpeedUp,
    SpeedDown,
    SpeedReset,
    Flip,
    Mirror,
    ToggleAntialias,
    Render,
    ToggleFullscreen,
    ToggleFilmstrip,
    /// The panel of file names down the left side.
    ToggleList,
    ToggleInfo,
    ToggleDebug,
    /// What `--verbose` and `--debug` print on the terminal.
    ToggleVerbose,
    ToggleDebugLog,
    ToggleExif,
    ToggleActions,
    TogglePointer,
    ToggleMenu,
    ToggleGrid,
    ToggleHelp,
    ToggleSlideshow,
    SizeToImage,
    /// Look for new and deleted files, and read the current one again.
    Reload,
    ToggleWatch,
    Remove,
    Trash,
    Delete,
    Save,
    SaveFilelist,
    Action(usize),
    Mark,
    MarkMenu,
    MarkAll,
    InvertMarks,
    ClearMarks,
    /// Narrow the list to the marked files.
    OnlyMarked,
    CopyName,
    CopyPath,
    CopyTo,
    MoveTo,
    Undo,
    Rename,
    Convert(Format),
    PlayPause,
    SeekForward,
    SeekBack,
    VolumeUp,
    VolumeDown,
    /// Play the video from its beginning.
    Restart,
    Search,
    ClearFilter,
    /// The `S` menu, and what it offers: an order for the list, a shuffle,
    /// and the same order backwards.
    SortMenu,
    SortBy(Sort),
    Shuffle,
    ReverseOrder,
    FrameForward,
    FrameBack,
    Quit,
}

impl Command {
    /// Whether holding the key down should keep triggering the command.
    pub fn repeats(self) -> bool {
        use Command::*;
        matches!(self, Next | Previous | JumpForward | JumpBack | ZoomIn | ZoomOut | Scroll(..))
    }
}

pub fn for_key(key: Key<&str>, modifiers: ModifiersState, numpad: bool) -> Option<Command> {
    use Command::*;
    let arrow = |dx, dy| {
        if modifiers.alt_key() {
            Some(Scroll(dx, dy, true))
        } else if modifiers.control_key() || numpad {
            Some(Scroll(dx, dy, false))
        } else {
            None
        }
    };
    Some(match key {
        Key::Named(named) => match named {
            NamedKey::ArrowRight => arrow(1, 0).unwrap_or(Next),
            NamedKey::ArrowLeft => arrow(-1, 0).unwrap_or(Previous),
            NamedKey::ArrowUp => arrow(0, -1).unwrap_or(ZoomIn),
            NamedKey::ArrowDown => arrow(0, 1).unwrap_or(ZoomOut),
            NamedKey::Space => Next,
            NamedKey::Backspace => Previous,
            NamedKey::Home => First,
            NamedKey::End => Last,
            NamedKey::PageDown => JumpForward,
            NamedKey::PageUp => JumpBack,
            NamedKey::Delete if modifiers.control_key() => Trash,
            NamedKey::Delete => Remove,
            NamedKey::Enter => Action(0),
            NamedKey::Escape => Quit,
            NamedKey::Tab => ToggleMenu,
            NamedKey::F2 => Rename,
            NamedKey::F5 => Reload,
            _ => return None,
        },
        Key::Character(text) => match text {
            ">" | "." | "n" => Next,
            "<" | "," => Previous,
            "p" => ToggleList,
            "z" => Random,
            "}" => NextDir,
            "{" => PreviousDir,
            "+" | "=" => ZoomIn,
            "-" => ZoomOut,
            "*" => ZoomActual,
            "0" => ZoomFit,
            "/" => Search,
            "!" => ZoomFill,
            "Z" => ToggleEnlarge,
            "k" => ToggleKeepView,
            "]" => SpeedUp,
            "[" => SpeedDown,
            "\\" => SpeedReset,
            ")" => RotateRight,
            "(" => RotateLeft,
            "_" => Flip,
            "|" => Mirror,
            "A" => ToggleAntialias,
            "R" => Render,
            "f" => ToggleFullscreen,
            "t" => ToggleFilmstrip,
            "d" => Delete,
            "i" => ToggleInfo,
            "I" => ToggleDebug,
            "e" => ToggleExif,
            "a" => ToggleActions,
            "o" => TogglePointer,
            "m" => MoveTo,
            "v" => CopyTo,
            "u" => Undo,
            "x" => Mark,
            "X" => MarkMenu,
            "c" => CopyName,
            "C" => CopyPath,
            "g" => ToggleGrid,
            "?" => ToggleHelp,
            "h" => ToggleSlideshow,
            "w" => SizeToImage,
            "r" => Reload,
            "s" => Save,
            "S" => SortMenu,
            "W" => ToggleWatch,
            "L" => SaveFilelist,
            "q" => Quit,
            digit => Action(digit.parse().ok().filter(|n| *n <= 9)?),
        },
        _ => return None,
    })
}

/// Which kind of file a menu item makes sense for.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Applies {
    Always,
    Images,
    Videos,
    /// Only while the list is narrowed to the matches of a search.
    Filtered,
}

pub struct MenuItem {
    pub label: &'static str,
    pub keys: &'static str,
    pub command: Command,
    pub applies: Applies,
    /// The menu this item opens beside it, instead of running `command`.
    /// Empty for an ordinary item.
    pub sub: &'static [Option<MenuItem>],
}

const fn item(label: &'static str, keys: &'static str, command: Command) -> Option<MenuItem> {
    Some(MenuItem { label, keys, command, applies: Applies::Always, sub: &[] })
}

const fn image_item(label: &'static str, keys: &'static str, command: Command) -> Option<MenuItem> {
    Some(MenuItem { label, keys, command, applies: Applies::Images, sub: &[] })
}

const fn video_item(label: &'static str, keys: &'static str, command: Command) -> Option<MenuItem> {
    Some(MenuItem { label, keys, command, applies: Applies::Videos, sub: &[] })
}

const fn submenu(label: &'static str, sub: &'static [Option<MenuItem>], applies: Applies) -> Option<MenuItem> {
    Some(MenuItem { label, keys: "\u{25B8}", command: Command::ToggleMenu, applies, sub })
}

/// The right-click menu. `None` is a separator.
pub const MENU: &[Option<MenuItem>] = &[
    image_item("Next", ">", Command::Next),
    image_item("Previous", "<", Command::Previous),
    video_item("Next", "Ctrl+Right", Command::Next),
    video_item("Previous", "Ctrl+Left", Command::Previous),
    item("Search...", "/", Command::Search),
    Some(MenuItem { label: "Show all files again", keys: "Esc", command: Command::ClearFilter, applies: Applies::Filtered, sub: &[] }),
    item("Random", "z", Command::Random),
    None,
    submenu("Playback", PLAYBACK, Applies::Videos),
    submenu("Zoom", ZOOM, Applies::Always),
    submenu("Rotate", ROTATE, Applies::Always),
    submenu("View", VIEW, Applies::Always),
    submenu("Sort", SORT, Applies::Always),
    submenu("Marks", MARKS, Applies::Always),
    submenu("File", FILE, Applies::Always),
    submenu("Debug", DEBUG, Applies::Always),
    None,
    item("Keyboard shortcuts", "?", Command::ToggleHelp),
    item("Quit", "q", Command::Quit),
];

const PLAYBACK: &[Option<MenuItem>] = &[
    video_item("Play / pause", "Space", Command::PlayPause),
    video_item("Forward 5 seconds", "Right", Command::SeekForward),
    video_item("Back 5 seconds", "Left", Command::SeekBack),
    video_item("Next frame", "Shift+Right", Command::FrameForward),
    video_item("Previous frame", "Shift+Left", Command::FrameBack),
    video_item("Play from the start", "", Command::Restart),
    None,
    video_item("Faster", "]", Command::SpeedUp),
    video_item("Slower", "[", Command::SpeedDown),
    video_item("Normal speed", "\\", Command::SpeedReset),
    None,
    video_item("Volume up", "Up", Command::VolumeUp),
    video_item("Volume down", "Down", Command::VolumeDown),
];

const ZOOM: &[Option<MenuItem>] = &[
    item("Zoom in", "+", Command::ZoomIn),
    item("Zoom out", "-", Command::ZoomOut),
    item("Actual size", "*", Command::ZoomActual),
    item("Fit to screen", "0", Command::ZoomFit),
    item("Fill screen", "!", Command::ZoomFill),
    item("Fit small images to screen", "Z", Command::ToggleEnlarge),
];

const ROTATE: &[Option<MenuItem>] = &[
    item("Rotate right", ")", Command::RotateRight),
    item("Rotate left", "(", Command::RotateLeft),
    item("Flip vertically", "_", Command::Flip),
    item("Mirror horizontally", "|", Command::Mirror),
];

const VIEW: &[Option<MenuItem>] = &[
    item("Fullscreen", "f", Command::ToggleFullscreen),
    item("Grid view", "g", Command::ToggleGrid),
    item("Filmstrip", "t", Command::ToggleFilmstrip),
    item("File list", "p", Command::ToggleList),
    item("Slideshow", "h", Command::ToggleSlideshow),
    item("Watch for new files", "W", Command::ToggleWatch),
    None,
    item("Info", "i", Command::ToggleInfo),
    image_item("EXIF data", "e", Command::ToggleExif),
];

/// Also the `S` menu.
pub const SORT: &[Option<MenuItem>] = &[
    item("Name", "", Command::SortBy(Sort::Name)),
    item("File name, ignoring folders", "", Command::SortBy(Sort::Filename)),
    item("Newest first", "", Command::SortBy(Sort::Mtime)),
    item("Smallest first", "", Command::SortBy(Sort::Size)),
    item("Shuffle", "", Command::Shuffle),
    None,
    item("Reverse", "", Command::ReverseOrder),
];

const MARKS: &[Option<MenuItem>] = &[
    item("Mark", "x", Command::Mark),
    item("Marked images...", "X", Command::MarkMenu),
    item("Show only marked", "", Command::OnlyMarked),
    item("Clear all marks", "", Command::ClearMarks),
];

const FILE: &[Option<MenuItem>] = &[
    item("Copy to folder", "v", Command::CopyTo),
    item("Move to folder", "m", Command::MoveTo),
    item("Rename", "F2", Command::Rename),
    image_item("Convert to JPEG", "", Command::Convert(Format::Jpeg)),
    image_item("Convert to PNG", "", Command::Convert(Format::Png)),
    image_item("Convert to WebP", "", Command::Convert(Format::WebP)),
    item("Undo", "u", Command::Undo),
    None,
    item("Copy file name", "c", Command::CopyName),
    item("Copy full path", "C", Command::CopyPath),
    None,
    item("Refresh", "r", Command::Reload),
    item("Remove from list", "Del", Command::Remove),
    item("Move to trash", "Ctrl+Del", Command::Trash),
    item("Delete...", "d", Command::Delete),
];

const DEBUG: &[Option<MenuItem>] = &[
    item("Timings on screen", "I", Command::ToggleDebug),
    item("Print files to terminal", "", Command::ToggleVerbose),
    item("Print timings to terminal", "", Command::ToggleDebugLog),
];

/// The `X` menu: what can be done with the marked images.
pub const MARK_MENU: &[Option<MenuItem>] = &[
    item("Copy to folder", "v", Command::CopyTo),
    item("Move to folder", "m", Command::MoveTo),
    None,
    item("Copy file names", "c", Command::CopyName),
    item("Copy full paths", "C", Command::CopyPath),
    item("Save list to a file", "L", Command::SaveFilelist),
    None,
    image_item("Convert to JPEG", "", Command::Convert(Format::Jpeg)),
    image_item("Convert to PNG", "", Command::Convert(Format::Png)),
    image_item("Convert to WebP", "", Command::Convert(Format::WebP)),
    None,
    item("Move to trash", "Ctrl+Del", Command::Trash),
    item("Delete...", "d", Command::Delete),
    None,
    item("Mark all", "", Command::MarkAll),
    item("Invert marks", "", Command::InvertMarks),
    item("Clear marks", "", Command::ClearMarks),
];

/// The buttons under the grid while images are marked.
pub const GRID_ACTIONS: &[(&str, Command)] = &[
    ("Copy  v", Command::CopyTo),
    ("Move  m", Command::MoveTo),
    ("Names  c", Command::CopyName),
    ("Delete  d", Command::Delete),
    ("More  X", Command::MarkMenu),
];

/// The `?` overlay: columns of sections, each a title and its key list.
pub const HELP: &[&[(&str, &[(&str, &str)])]] = &[
    &[
        (
            "Navigation",
            &[
                (">  .  n  Right  Space", "Next image"),
                ("<  ,  Left  Backspace", "Previous image"),
                ("Home  End", "First / last image"),
                ("PgDn  PgUp", "Jump 5% forward / back"),
                ("z", "Random image (in the grid: shuffle)"),
                ("{  }", "Previous / next directory"),
                ("S", "Sort: name, newest, size, shuffle, reverse"),
                ("Wheel", "Next / previous image"),
                ("/", "Search: Enter goes, Tab keeps matches"),
                ("", "is:video  is:marked  >10mb  ^start  !without"),
                ("Esc", "Clear a kept search, else quit"),
            ],
        ),
        (
            "Zoom and pan",
            &[
                ("+  Up", "Zoom in"),
                ("-  Down", "Zoom out"),
                ("*", "Actual size"),
                ("0", "Fit to screen"),
                ("!", "Fill screen"),
                ("Z", "Small images: fit screen / actual size"),
                ("k", "Keep zoom between images"),
                ("Ctrl+arrows", "Scroll (left / right: files, if it all fits)"),
                ("Alt+arrows", "Scroll by a page"),
                ("Drag", "Pan"),
                ("Middle-drag", "Zoom (click for actual size)"),
                ("Ctrl+wheel", "Zoom at the pointer"),
            ],
        ),
        (
            "Rotate",
            &[
                (")  (", "Rotate right / left"),
                ("_", "Flip vertically"),
                ("|", "Mirror horizontally"),
            ],
        ),
    ],
    &[
        (
            "Views",
            &[
                ("g", "Grid view (arrows move, Enter opens)"),
                ("t", "Filmstrip (or pointer at bottom edge)"),
                ("p", "File list down the side (click to go)"),
                ("f  Double click", "Fullscreen"),
                ("Tab  Right click", "Menu"),
                ("?", "This list"),
                ("i", "Info: name, size, zoom, kept search"),
                ("I", "Debug: decode, draw and playback timings"),
                ("e", "EXIF data"),
                ("a", "Actions"),
                ("o", "Mouse pointer"),
                ("A", "Smooth scaling"),
                ("w", "Size window to image"),
            ],
        ),
        (
            "Files",
            &[
                ("x", "Mark / unmark"),
                ("X", "Menu of things to do with the marked"),
                ("c  C", "Copy file name / full path"),
                ("v  m", "Copy / move to the -o folder"),
                ("u", "Undo copy, move, rename or convert"),
                ("F2", "Rename"),
                ("r  F5", "Refresh: new and deleted files"),
                ("W", "Watch: refresh as files come and go"),
                ("Delete", "Remove from list"),
                ("Ctrl+Delete", "Move to trash"),
                ("d", "Delete for good (asks first)"),
                ("s", "Save a copy here"),
                ("L", "Save the file list here"),
                ("Enter  1-9", "Run action"),
            ],
        ),
        (
            "While a video plays",
            &[
                ("Space", "Play / pause"),
                ("Left  Right", "Seek 5 seconds; faster while held"),
                ("PgUp  PgDn", "Seek back / forward 1 minute"),
                ("Pointer at the bottom", "Seek bar: hover to preview, click to jump"),
                ("Shift+Left  Shift+Right", "Previous / next frame"),
                ("Up  Down", "Volume"),
                ("]  [  \\", "Faster / slower / normal speed"),
                ("Ctrl+Left  Ctrl+Right", "Previous / next file"),
                (",  .  <  >  n", "Previous / next file"),
                ("At the end", "The next file plays"),
            ],
        ),
        (
            "Other",
            &[
                ("h", "Start / pause slideshow"),
                ("q  Esc", "Quit"),
            ],
        ),
    ],
];
