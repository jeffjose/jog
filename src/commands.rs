use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::convert::Format;

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
    Flip,
    Mirror,
    ToggleAntialias,
    Render,
    ToggleFullscreen,
    ToggleFilmstrip,
    ToggleInfo,
    ToggleExif,
    ToggleActions,
    TogglePointer,
    ToggleMenu,
    ToggleGrid,
    ToggleHelp,
    ToggleSlideshow,
    SizeToImage,
    Reload,
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
            "<" | "," | "p" => Previous,
            "z" => Random,
            "}" => NextDir,
            "{" => PreviousDir,
            "+" | "=" => ZoomIn,
            "-" => ZoomOut,
            "*" => ZoomActual,
            "/" => ZoomFit,
            "!" => ZoomFill,
            "Z" => ToggleEnlarge,
            "k" => ToggleKeepView,
            "]" => RotateRight,
            "[" => RotateLeft,
            "_" => Flip,
            "|" => Mirror,
            "A" => ToggleAntialias,
            "R" => Render,
            "f" => ToggleFullscreen,
            "t" => ToggleFilmstrip,
            "d" => Delete,
            "i" => ToggleInfo,
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
            "r" => Rename,
            "s" => Save,
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
}

pub struct MenuItem {
    pub label: &'static str,
    pub keys: &'static str,
    pub command: Command,
    pub applies: Applies,
}

const fn item(label: &'static str, keys: &'static str, command: Command) -> Option<MenuItem> {
    Some(MenuItem { label, keys, command, applies: Applies::Always })
}

const fn image_item(label: &'static str, keys: &'static str, command: Command) -> Option<MenuItem> {
    Some(MenuItem { label, keys, command, applies: Applies::Images })
}

const fn video_item(label: &'static str, keys: &'static str, command: Command) -> Option<MenuItem> {
    Some(MenuItem { label, keys, command, applies: Applies::Videos })
}

/// The right-click menu. `None` is a separator.
pub const MENU: &[Option<MenuItem>] = &[
    image_item("Next", ">", Command::Next),
    image_item("Previous", "<", Command::Previous),
    video_item("Next", "Right", Command::Next),
    video_item("Previous", "Left", Command::Previous),
    item("Random", "z", Command::Random),
    None,
    video_item("Play / pause", "Space", Command::PlayPause),
    video_item("Forward 5 seconds", ">", Command::SeekForward),
    video_item("Back 5 seconds", "<", Command::SeekBack),
    video_item("Volume up", "Up", Command::VolumeUp),
    video_item("Volume down", "Down", Command::VolumeDown),
    image_item("Zoom in", "+", Command::ZoomIn),
    image_item("Zoom out", "-", Command::ZoomOut),
    image_item("Actual size", "*", Command::ZoomActual),
    image_item("Fit to screen", "/", Command::ZoomFit),
    image_item("Fill screen", "!", Command::ZoomFill),
    image_item("Fit small images to screen", "Z", Command::ToggleEnlarge),
    None,
    image_item("Rotate right", "]", Command::RotateRight),
    image_item("Rotate left", "[", Command::RotateLeft),
    image_item("Flip vertically", "_", Command::Flip),
    image_item("Mirror horizontally", "|", Command::Mirror),
    None,
    item("Fullscreen", "f", Command::ToggleFullscreen),
    item("Grid view", "g", Command::ToggleGrid),
    item("Filmstrip", "t", Command::ToggleFilmstrip),
    item("Info", "i", Command::ToggleInfo),
    image_item("EXIF data", "e", Command::ToggleExif),
    item("Slideshow", "h", Command::ToggleSlideshow),
    None,
    item("Mark", "x", Command::Mark),
    item("Marked images...", "X", Command::MarkMenu),
    item("Clear all marks", "", Command::ClearMarks),
    item("Copy file name", "c", Command::CopyName),
    item("Copy full path", "C", Command::CopyPath),
    None,
    item("Copy to folder", "v", Command::CopyTo),
    item("Move to folder", "m", Command::MoveTo),
    item("Rename", "r", Command::Rename),
    image_item("Convert to JPEG", "", Command::Convert(Format::Jpeg)),
    image_item("Convert to PNG", "", Command::Convert(Format::Png)),
    image_item("Convert to WebP", "", Command::Convert(Format::WebP)),
    item("Undo", "u", Command::Undo),
    None,
    item("Reload", "F5", Command::Reload),
    item("Remove from list", "Del", Command::Remove),
    item("Move to trash", "Ctrl+Del", Command::Trash),
    item("Delete...", "d", Command::Delete),
    None,
    item("Keyboard shortcuts", "?", Command::ToggleHelp),
    item("Quit", "q", Command::Quit),
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
                ("<  ,  p  Left  Backspace", "Previous image"),
                ("Home  End", "First / last image"),
                ("PgDn  PgUp", "Jump 5% forward / back"),
                ("z", "Random image (in the grid: shuffle)"),
                ("{  }", "Previous / next directory"),
                ("Wheel", "Next / previous image"),
            ],
        ),
        (
            "Zoom and pan",
            &[
                ("+  Up", "Zoom in"),
                ("-  Down", "Zoom out"),
                ("*", "Actual size"),
                ("/", "Fit to screen"),
                ("!", "Fill screen"),
                ("Z", "Small images: fit screen / actual size"),
                ("k", "Keep zoom between images"),
                ("Ctrl+arrows", "Scroll"),
                ("Alt+arrows", "Scroll by a page"),
                ("Drag", "Pan"),
                ("Middle-drag", "Zoom (click for actual size)"),
                ("Ctrl+wheel", "Zoom at the pointer"),
            ],
        ),
        (
            "Rotate",
            &[
                ("]  [", "Rotate right / left"),
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
                ("f", "Fullscreen"),
                ("Tab  Right click", "Menu"),
                ("?", "This list"),
                ("i", "Info: name, dimensions, zoom, size"),
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
                ("r  F2", "Rename"),
                ("F5", "Reload"),
                ("Delete", "Remove from list"),
                ("Ctrl+Delete", "Move to trash"),
                ("d", "Delete for good (asks first)"),
                ("s", "Save a copy here"),
                ("L", "Save the file list here"),
                ("Enter  0-9", "Run action"),
            ],
        ),
        (
            "While a video plays",
            &[
                ("Space", "Play / pause"),
                ("<  >", "Seek back / forward 5 seconds"),
                ("Up  Down", "Volume"),
                ("Left  Right  n  p", "Previous / next file"),
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
