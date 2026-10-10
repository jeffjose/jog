use winit::keyboard::{Key, ModifiersState, NamedKey};

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
    ToggleFilename,
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
    Save,
    SaveFilelist,
    Action(usize),
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
            "d" => ToggleFilename,
            "i" => ToggleInfo,
            "e" => ToggleExif,
            "a" => ToggleActions,
            "o" => TogglePointer,
            "m" => ToggleMenu,
            "g" => ToggleGrid,
            "?" => ToggleHelp,
            "h" => ToggleSlideshow,
            "w" => SizeToImage,
            "r" => Reload,
            "s" => Save,
            "L" => SaveFilelist,
            "q" | "x" => Quit,
            digit => Action(digit.parse().ok().filter(|n| *n <= 9)?),
        },
        _ => return None,
    })
}

pub struct MenuItem {
    pub label: &'static str,
    pub keys: &'static str,
    pub command: Command,
}

const fn item(label: &'static str, keys: &'static str, command: Command) -> Option<MenuItem> {
    Some(MenuItem { label, keys, command })
}

/// The right-click menu. `None` is a separator.
pub const MENU: &[Option<MenuItem>] = &[
    item("Next image", ">", Command::Next),
    item("Previous image", "<", Command::Previous),
    item("Random image", "z", Command::Random),
    None,
    item("Zoom in", "+", Command::ZoomIn),
    item("Zoom out", "-", Command::ZoomOut),
    item("Actual size", "*", Command::ZoomActual),
    item("Fit to screen", "/", Command::ZoomFit),
    item("Fill screen", "!", Command::ZoomFill),
    item("Enlarge small images", "Z", Command::ToggleEnlarge),
    None,
    item("Rotate right", "]", Command::RotateRight),
    item("Rotate left", "[", Command::RotateLeft),
    item("Flip vertically", "_", Command::Flip),
    item("Mirror horizontally", "|", Command::Mirror),
    None,
    item("Fullscreen", "f", Command::ToggleFullscreen),
    item("Grid view", "g", Command::ToggleGrid),
    item("Filmstrip", "t", Command::ToggleFilmstrip),
    item("File name", "d", Command::ToggleFilename),
    item("Info", "i", Command::ToggleInfo),
    item("EXIF data", "e", Command::ToggleExif),
    item("Slideshow", "h", Command::ToggleSlideshow),
    None,
    item("Reload", "r", Command::Reload),
    item("Remove from list", "Del", Command::Remove),
    item("Move to trash", "Ctrl+Del", Command::Trash),
    None,
    item("Keyboard shortcuts", "?", Command::ToggleHelp),
    item("Quit", "q", Command::Quit),
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
                ("Z", "Enlarge small images"),
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
                ("m  Right click", "Menu"),
                ("?", "This list"),
                ("d", "File name"),
                ("i", "Info"),
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
                ("r", "Reload"),
                ("Delete", "Remove from list"),
                ("Ctrl+Delete", "Move to trash"),
                ("s", "Save a copy here"),
                ("L", "Save the file list here"),
                ("Enter  0-9", "Run action"),
            ],
        ),
        (
            "Other",
            &[
                ("h", "Start / pause slideshow"),
                ("q  x  Esc", "Quit"),
            ],
        ),
    ],
];
