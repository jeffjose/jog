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
    /// In the grid: open the file under the cursor, show its pictures
    /// larger or smaller, name them or not, and the menu for a cell.
    OpenFromGrid,
    /// The same for a video, which it plays.
    PlayFromGrid,
    GridLarger,
    GridSmaller,
    ToggleGridTitles,
    GridMenu,
    ToggleMute,
    /// Step through no looping, this video over and over, and the whole
    /// list over and over.
    CycleLoop,
    /// Seek to the exact moment asked for, or to the nearest keyframe.
    TogglePrecise,
    /// Show the scenes of the video on view along the bottom, or stop.
    ToggleScenes,
    /// Step through a video's subtitle tracks, and after the last none.
    CycleSubtitles,
    /// Step through a video's audio tracks.
    CycleAudio,
    SubtitlesLarger,
    SubtitlesSmaller,
    /// Put the debug overlay's figures on the clipboard, plainly or as JSON.
    CopyDebug,
    CopyDebugJson,
    /// Go to a workspace by its number, counted from 0, or to the one after
    /// or before; list them to pick from; save the folder on view as one.
    Workspace(usize),
    /// Go to the workspace that has this digit for its key.
    WorkspaceKey(usize),
    NextWorkspace,
    PrevWorkspace,
    /// Back to where jog was started: what was named on the command line.
    HomeWorkspace,
    WorkspaceList,
    /// The overview: every workspace as a card, to look over and pick from.
    Overview,
    SaveWorkspace,
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
    /// The same, into a folder typed there and then.
    CopyToAsk,
    MoveToAsk,
    Undo,
    Rename,
    Convert(Format),
    PlayPause,
    SeekForward,
    SeekBack,
    /// Go to this many seconds into the video.
    SeekTo(f64),
    VolumeUp,
    VolumeDown,
    /// Play the video from its beginning.
    Restart,
    Search,
    ClearFilter,
    /// Go into deciding about files one after another, or come out of it.
    Triage,
    /// Type a shell command to run on the file on view, or on the marked
    /// ones: the find box, set to that.
    Shell,
    /// Run what was typed there, or the command at this place in the list
    /// of those run before, or the last one run once more.
    RunTyped,
    RunShell(usize),
    RepeatShell,
    /// In the grid, go back to the video that is playing.
    ShowPlaying,
    /// Type a search into the heading of the file list.
    ListSearch,
    /// Stand the file list beside the picture, or over it again; and move
    /// it to the other side of the window.
    ListPin,
    ListFlip,
    /// Drop a kept search without going back to where it was begun.
    ShowAllHere,
    /// The go-to box: type part of a name, pick a match, go to it.
    Palette,
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
            NamedKey::Tab if modifiers.control_key() && modifiers.shift_key() => PrevWorkspace,
            NamedKey::Tab if modifiers.control_key() => NextWorkspace,
            NamedKey::Tab => ToggleMenu,
            NamedKey::F2 => Rename,
            NamedKey::F5 => Reload,
            _ => return None,
        },
        Key::Character(text) => match text {
            ">" | "n" => Next,
            "." => RepeatShell,
            "<" | "," => Previous,
            "p" => ToggleList,
            "z" => Shuffle,
            "}" => NextDir,
            "{" => PreviousDir,
            "+" | "=" => ZoomIn,
            "-" => ZoomOut,
            "*" => ZoomActual,
            "0" => ZoomFit,
            "/" => Search,
            "@" => WorkspaceList,
            "`" | "~" if modifiers.control_key() => HomeWorkspace,
            "!" => Shell,
            "Q" => Triage,
            "F" => ZoomFill,
            "Z" => ToggleEnlarge,
            "k" if modifiers.control_key() || modifiers.super_key() => Palette,
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
            "D" => Delete,
            "i" => ToggleInfo,
            "I" => ToggleDebug,
            "e" => ToggleExif,
            "a" => ToggleActions,
            "o" => TogglePointer,
            "M" => MoveTo,
            "V" => CopyTo,
            "u" => Undo,
            "x" => Mark,
            "X" => MarkMenu,
            "c" => CopyName,
            "C" => CopyPath,
            "g" => ToggleGrid,
            "G" => Overview,
            "?" => ToggleHelp,
            "h" => ToggleSlideshow,
            "w" => SizeToImage,
            "r" => Reload,
            "S" => Save,
            "s" => SortMenu,
            "m" => ToggleMute,
            "l" => CycleLoop,
            "T" => ToggleScenes,
            "j" => CycleSubtitles,
            "J" => CycleAudio,
            "b" => Restart,
            "W" => ToggleWatch,
            "L" => SaveFilelist,
            "q" => Quit,
            // Ctrl with a digit is a workspace; a digit alone, an action.
            digit if modifiers.control_key() => WorkspaceKey(digit.parse::<usize>().ok().filter(|n| (1..=9).contains(n))?),
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
    Some(MenuItem { label, keys: icons::CHEVRON_RIGHT, command: Command::ToggleMenu, applies, sub })
}

/// Icons from Solar (480 Design), in its line style at a heavier line,
/// which jog carries as a font made by `tools/solar_font.py`: each is
/// the letter that font has it at.
pub mod icons {
    pub const ARROW_RIGHT: &str = "\u{e04d}";
    pub const ARROW_LEFT: &str = "\u{e04c}";
    pub const SEARCH: &str = "\u{e154}";
    pub const X: &str = "\u{e1b1}";
    pub const SHUFFLE: &str = "\u{e161}";
    pub const A_TO_Z: &str = "\u{e419}";
    pub const CLOCK: &str = "\u{e08b}";
    pub const WEIGHT: &str = "\u{e534}";
    pub const PLAY: &str = "\u{e13f}";
    pub const PAUSE: &str = "\u{e131}";
    pub const ROTATE_CCW: &str = "\u{e14b}";
    pub const ROTATE_CW: &str = "\u{e14c}";
    pub const CHEVRONS_RIGHT: &str = "\u{e077}";
    pub const CHEVRONS_LEFT: &str = "\u{e076}";
    pub const GAUGE: &str = "\u{e1be}";
    pub const VOLUME_2: &str = "\u{e1aa}";
    pub const VOLUME_1: &str = "\u{e1a9}";
    pub const VOLUME_X: &str = "\u{e1ab}";
    pub const REPEAT: &str = "\u{e149}";
    pub const REPEAT_1: &str = "\u{e1fc}";
    pub const PIN: &str = "\u{f000}";
    pub const CLAPPERBOARD: &str = "\u{e29a}";
    pub const CAPTIONS: &str = "\u{e3a7}";
    pub const CAPTIONS_OFF: &str = "\u{e5c5}";
    pub const AUDIO_LINES: &str = "\u{e55e}";
    pub const A_LARGE_SMALL: &str = "\u{e58b}";
    pub const ZOOM_IN: &str = "\u{e1b5}";
    pub const ZOOM_OUT: &str = "\u{e1b6}";
    pub const SCAN: &str = "\u{e256}";
    pub const MAXIMIZE: &str = "\u{e115}";
    pub const EXPAND: &str = "\u{e219}";
    pub const SHRINK: &str = "\u{e21f}";
    pub const FULLSCREEN: &str = "\u{e538}";
    pub const FLIP_VERTICAL: &str = "\u{e362}";
    pub const FLIP_HORIZONTAL: &str = "\u{e360}";
    pub const LAYOUT_GRID: &str = "\u{e104}";
    pub const FILM: &str = "\u{e0d4}";
    pub const LIST: &str = "\u{e10b}";
    pub const IMAGES: &str = "\u{e5c8}";
    pub const EYE: &str = "\u{e0be}";
    pub const INFO: &str = "\u{e0fe}";
    pub const CAMERA: &str = "\u{e068}";
    pub const ARROW_UP_DOWN: &str = "\u{e380}";
    pub const SORT: &str = "\u{e04b}";
    pub const STAR: &str = "\u{e179}";
    pub const STAR_OFF: &str = "\u{e2af}";
    pub const FLAG: &str = "\u{e0d6}";
    pub const COPY: &str = "\u{e0a2}";
    pub const BRACES: &str = "\u{e36d}";
    pub const FOLDER_INPUT: &str = "\u{e337}";
    pub const FOLDER_PLUS: &str = "\u{e0de}";
    pub const FOLDERS: &str = "\u{e342}";
    pub const PENCIL: &str = "\u{e1f8}";
    pub const REFRESH: &str = "\u{e148}";
    pub const UNDO: &str = "\u{e2a0}";
    pub const CLIPBOARD: &str = "\u{e089}";
    pub const MINUS: &str = "\u{e11f}";
    pub const TRASH: &str = "\u{e18d}";
    pub const CIRCLE_X: &str = "\u{e088}";
    pub const BUG: &str = "\u{e20b}";
    pub const TERMINAL: &str = "\u{e184}";
    pub const KEYBOARD: &str = "\u{e283}";
    pub const LOG_OUT: &str = "\u{e111}";
    pub const EXTERNAL_LINK: &str = "\u{e0bd}";
    pub const CHECK: &str = "\u{e070}";
    pub const CHEVRON_RIGHT: &str = "\u{e073}";
    pub const CHEVRON_LEFT: &str = "\u{e072}";
    pub const TRIANGLE_ALERT: &str = "\u{e192}";
    pub const TYPE: &str = "\u{e197}";
    pub const MONITOR_PLAY: &str = "\u{e489}";
    pub const FILE: &str = "\u{e0c4}";
    pub const STEP_FORWARD: &str = "\u{e3ed}";
    pub const STEP_BACK: &str = "\u{e3ec}";
    pub const FAST_FORWARD: &str = "\u{e0c1}";
    pub const REWIND: &str = "\u{e14a}";
    pub const SKIP_BACK: &str = "\u{e162}";
    pub const SKIP_FORWARD: &str = "\u{e163}";
    pub const REPLACE: &str = "\u{e3de}";
    pub const LIST_FILTER: &str = "\u{e464}";
    pub const LIST_X: &str = "\u{e23f}";
}

/// The icon drawn beside a menu item.
pub fn icon(item: &MenuItem) -> &'static str {
    use icons::*;
    if !item.sub.is_empty() {
        return match item.label {
            "Playback" => MONITOR_PLAY,
            "Zoom" => ZOOM_IN,
            "Rotate" => ROTATE_CW,
            "View" => EYE,
            "Sort" => SORT,
            "Marks" => STAR,
            "File" => FILE,
            "Debug" => BUG,
            _ => "",
        };
    }
    command_icon(item.command)
}

/// The icon for a command, wherever it is offered.
pub fn command_icon(command: Command) -> &'static str {
    use icons::*;
    use Command::*;
    match command {
        Next => ARROW_RIGHT,
        Previous => ARROW_LEFT,
        Search | Palette => SEARCH,
        WorkspaceList | Overview | Workspace(_) | WorkspaceKey(_) | NextWorkspace | PrevWorkspace | HomeWorkspace => FOLDERS,
        SaveWorkspace => FOLDER_PLUS,
        ClearFilter | ShowAllHere => LIST_X,
        Triage => CHECK,
        Shell | RunTyped | RunShell(_) | RepeatShell => TERMINAL,
        ListSearch => SEARCH,
        ListPin => PIN,
        ListFlip => REPLACE,
        ClearMarks => STAR_OFF,
        Shuffle => SHUFFLE,
        PlayPause => PLAY,
        ShowPlaying => LAYOUT_GRID,
        SeekForward => FAST_FORWARD,
        SeekBack => REWIND,
        FrameForward => STEP_FORWARD,
        FrameBack => STEP_BACK,
        Restart => SKIP_BACK,
        SpeedUp => CHEVRONS_RIGHT,
        SpeedDown => CHEVRONS_LEFT,
        SpeedReset => GAUGE,
        VolumeUp => VOLUME_2,
        VolumeDown => VOLUME_1,
        ToggleMute => VOLUME_X,
        CycleLoop => REPEAT,
        TogglePrecise => SCAN,
        ToggleScenes => CLAPPERBOARD,
        CycleSubtitles => CAPTIONS,
        CycleAudio => AUDIO_LINES,
        SubtitlesLarger | SubtitlesSmaller => A_LARGE_SMALL,
        ZoomIn | GridLarger => ZOOM_IN,
        ZoomOut | GridSmaller => ZOOM_OUT,
        ZoomActual => SCAN,
        ZoomFit => SHRINK,
        ZoomFill => EXPAND,
        ToggleEnlarge => MAXIMIZE,
        RotateRight => ROTATE_CW,
        RotateLeft => ROTATE_CCW,
        Flip => FLIP_VERTICAL,
        Mirror => FLIP_HORIZONTAL,
        ToggleFullscreen => FULLSCREEN,
        ToggleGrid => LAYOUT_GRID,
        ToggleFilmstrip => FILM,
        ToggleList => LIST,
        ToggleSlideshow => IMAGES,
        ToggleWatch => EYE,
        ToggleInfo => INFO,
        ToggleExif => CAMERA,
        ToggleGridTitles => TYPE,
        SortBy(_) | SortMenu => SORT,
        ReverseOrder => ARROW_UP_DOWN,
        Mark | MarkAll => STAR,
        MarkMenu | InvertMarks => FLAG,
        OnlyMarked => LIST_FILTER,
        CopyTo | CopyToAsk => FOLDER_PLUS,
        MoveTo | MoveToAsk => FOLDER_INPUT,
        Rename => PENCIL,
        Convert(_) => REPLACE,
        Undo => UNDO,
        CopyName | SaveFilelist => COPY,
        CopyPath => CLIPBOARD,
        Reload => REFRESH,
        Remove => MINUS,
        Trash => TRASH,
        Delete => CIRCLE_X,
        ToggleDebug => BUG,
        ToggleVerbose | ToggleDebugLog => TERMINAL,
        ToggleHelp => KEYBOARD,
        OpenFromGrid => EXTERNAL_LINK,
        PlayFromGrid => PLAY,
        Quit => LOG_OUT,
        _ => "",
    }
}

/// The right-click menu. `None` is a separator.
pub const MENU: &[Option<MenuItem>] = &[
    image_item("Next", ">", Command::Next),
    image_item("Previous", "<", Command::Previous),
    video_item("Next", "Ctrl+Right", Command::Next),
    video_item("Previous", "Ctrl+Left", Command::Previous),
    item("Find...", "/", Command::Search),
    Some(MenuItem { label: "Back from this search", keys: "Esc", command: Command::ClearFilter, applies: Applies::Filtered, sub: &[] }),
    Some(MenuItem { label: "Show all files, stay on this one", keys: "", command: Command::ShowAllHere, applies: Applies::Filtered, sub: &[] }),
    item("Shuffle", "z", Command::Shuffle),
    item("Workspaces", "G", Command::Overview),
    item("Go to workspace...", "@", Command::WorkspaceList),
    item("Run a command on it...", "!", Command::Shell),
    item("Triage: keep or reject, file by file", "Q", Command::Triage),
    item("Run the last command again", ".", Command::RepeatShell),
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
    video_item("Back to the start", "b", Command::Restart),
    None,
    video_item("Faster", "]", Command::SpeedUp),
    video_item("Slower", "[", Command::SpeedDown),
    video_item("Normal speed", "\\", Command::SpeedReset),
    None,
    video_item("Volume up", "Up", Command::VolumeUp),
    video_item("Volume down", "Down", Command::VolumeDown),
    video_item("Mute", "m", Command::ToggleMute),
    None,
    video_item("Loop: off, this video, all", "l", Command::CycleLoop),
    video_item("Scenes", "T", Command::ToggleScenes),
    video_item("Precise seeking", "", Command::TogglePrecise),
    None,
    video_item("Next subtitles, or none", "j", Command::CycleSubtitles),
    video_item("Larger subtitles", "", Command::SubtitlesLarger),
    video_item("Smaller subtitles", "", Command::SubtitlesSmaller),
    video_item("Next audio track", "J", Command::CycleAudio),
];

const ZOOM: &[Option<MenuItem>] = &[
    item("Zoom in", "+", Command::ZoomIn),
    item("Zoom out", "-", Command::ZoomOut),
    item("Actual size", "*", Command::ZoomActual),
    item("Fit to screen", "0", Command::ZoomFit),
    item("Fill screen", "F", Command::ZoomFill),
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
    item("Newest first", "", Command::SortBy(Sort::Newest)),
    item("Oldest first", "", Command::SortBy(Sort::Oldest)),
    item("Largest first", "", Command::SortBy(Sort::Largest)),
    item("Smallest first", "", Command::SortBy(Sort::Smallest)),
    item("Shuffle", "z", Command::Shuffle),
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
    item("Copy to folder", "V", Command::CopyTo),
    item("Move to folder", "M", Command::MoveTo),
    item("Copy to...", "", Command::CopyToAsk),
    item("Move to...", "", Command::MoveToAsk),
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
    item("Delete...", "D", Command::Delete),
    None,
    item("Save folder as workspace...", "", Command::SaveWorkspace),
];

const DEBUG: &[Option<MenuItem>] = &[
    item("Timings on screen", "I", Command::ToggleDebug),
    item("Print files to terminal", "", Command::ToggleVerbose),
    item("Print timings to terminal", "", Command::ToggleDebugLog),
];

/// The menu for a file in the grid, on a right click or Tab.
pub const GRID_MENU: &[Option<MenuItem>] = &[
    video_item("Play", "Enter", Command::PlayFromGrid),
    image_item("Open", "Enter", Command::OpenFromGrid),
    item("Mark", "x", Command::Mark),
    None,
    item("Copy to folder", "V", Command::CopyTo),
    item("Move to folder", "M", Command::MoveTo),
    item("Copy to...", "", Command::CopyToAsk),
    item("Move to...", "", Command::MoveToAsk),
    item("Rename", "F2", Command::Rename),
    image_item("Convert to JPEG", "", Command::Convert(Format::Jpeg)),
    None,
    item("Copy file name", "c", Command::CopyName),
    item("Copy full path", "C", Command::CopyPath),
    None,
    item("Move to trash", "Ctrl+Del", Command::Trash),
    item("Delete...", "D", Command::Delete),
    None,
    item("Find...", "/", Command::Search),
    submenu("Sort", SORT, Applies::Always),
    submenu("Marks", MARKS, Applies::Always),
    None,
    item("Larger pictures", "Ctrl++", Command::GridLarger),
    item("Smaller pictures", "Ctrl+-", Command::GridSmaller),
    item("Titles", "i", Command::ToggleGridTitles),
    item("Close the grid", "g", Command::ToggleGrid),
];

/// The menu for a row of the file list, on a right click: what the grid
/// has for a picture, without what is only of the grid.
pub const LIST_MENU: &[Option<MenuItem>] = &[
    video_item("Play", "Enter", Command::PlayFromGrid),
    image_item("Open", "Enter", Command::OpenFromGrid),
    item("Mark", "x", Command::Mark),
    None,
    item("Copy to folder", "V", Command::CopyTo),
    item("Move to folder", "M", Command::MoveTo),
    item("Copy to...", "", Command::CopyToAsk),
    item("Move to...", "", Command::MoveToAsk),
    item("Rename", "F2", Command::Rename),
    image_item("Convert to JPEG", "", Command::Convert(Format::Jpeg)),
    None,
    item("Copy file name", "c", Command::CopyName),
    item("Copy full path", "C", Command::CopyPath),
    None,
    item("Move to trash", "Ctrl+Del", Command::Trash),
    item("Delete...", "D", Command::Delete),
    None,
    item("Find...", "/", Command::Search),
    submenu("Sort", SORT, Applies::Always),
    submenu("Marks", MARKS, Applies::Always),
];

/// The menu for the grid as a whole, on a right click off the pictures.
pub const GRID_AROUND_MENU: &[Option<MenuItem>] = &[
    item("Find...", "/", Command::Search),
    submenu("Sort", SORT, Applies::Always),
    item("Shuffle", "z", Command::Shuffle),
    item("Workspaces", "G", Command::Overview),
    item("Go to workspace...", "@", Command::WorkspaceList),
    item("Run a command on it...", "!", Command::Shell),
    item("Triage: keep or reject, file by file", "Q", Command::Triage),
    item("Run the last command again", ".", Command::RepeatShell),
    item("Save folder as workspace...", "", Command::SaveWorkspace),
    None,
    item("Mark all", "", Command::MarkAll),
    item("Invert marks", "", Command::InvertMarks),
    item("Clear all marks", "", Command::ClearMarks),
    item("Show only marked", "", Command::OnlyMarked),
    Some(MenuItem { label: "Back from this search", keys: "Esc", command: Command::ClearFilter, applies: Applies::Filtered, sub: &[] }),
    Some(MenuItem { label: "Show all files, stay on this one", keys: "", command: Command::ShowAllHere, applies: Applies::Filtered, sub: &[] }),
    None,
    item("Larger pictures", "Ctrl++", Command::GridLarger),
    item("Smaller pictures", "Ctrl+-", Command::GridSmaller),
    item("Titles", "i", Command::ToggleGridTitles),
    None,
    item("Refresh", "r", Command::Reload),
    item("Watch for new files", "W", Command::ToggleWatch),
    None,
    submenu("Debug", DEBUG, Applies::Always),
    item("Keyboard shortcuts", "?", Command::ToggleHelp),
    None,
    item("Close the grid", "g", Command::ToggleGrid),
    item("Quit", "q", Command::Quit),
];

/// The `X` menu: what can be done with the marked images.
pub const MARK_MENU: &[Option<MenuItem>] = &[
    item("Copy to folder", "V", Command::CopyTo),
    item("Move to folder", "M", Command::MoveTo),
    item("Copy to...", "", Command::CopyToAsk),
    item("Move to...", "", Command::MoveToAsk),
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
    item("Delete...", "D", Command::Delete),
    None,
    item("Mark all", "", Command::MarkAll),
    item("Invert marks", "", Command::InvertMarks),
    item("Clear marks", "", Command::ClearMarks),
];

/// The buttons under the grid while images are marked.
pub const GRID_ACTIONS: &[(&str, Command)] = &[
    ("Copy to folder  V", Command::CopyTo),
    ("Move to folder  M", Command::MoveTo),
    ("Copy names  c", Command::CopyName),
    ("Delete  D", Command::Delete),
    ("More  X", Command::MarkMenu),
];

/// The `?` overlay: columns of sections, each a title and its key list.
/// Keys are separated by two spaces; each is drawn as a key cap, unless
/// it is a few words describing something that is not a key.
pub const HELP: &[&[(&str, &[(&str, &str)])]] = &[
    &[
        (
            "Moving around",
            &[
                ("Right  Space  n", "Next file"),
                ("Left  Backspace  ,", "Previous file"),
                ("Home  End", "First / last file"),
                ("PgDn  PgUp", "Jump 5% forward / back"),
                ("}  {", "Next / previous folder"),
                ("Wheel", "Next / previous file"),
                ("z", "Shuffle: on to another file, or around this one in the grid, filmstrip or list"),
                ("!  .", "Run a shell command on this file, or on the marked ones / the last one again"),
                ("Q", "Triage: Space keeps, x rejects, each going on to the next; Q again to finish"),
                ("s", "Sort: name, newest, size, reverse"),
            ],
        ),
        (
            "Finding",
            &[
                ("/  Ctrl+K", "Open the find box"),
                ("Enter", "Go to the match picked"),
                ("Tab  Enter", "To the buttons: show only the matches"),
                ("Esc", "Leave a search, back to where it was begun"),
                ("G", "Workspaces: look over them all"),
                ("@  Ctrl+1  Ctrl+Tab", "Workspaces: list, go to, next"),
                ("Ctrl+`", "Back to what jog was started on"),
                ("word  ^start  !not", "Match names"),
                ("#video  #marked  >10mb", "Match kinds and sizes"),
                (">sort", "Find a command and run it"),
                (":42  :1:30  :50%", "Go to a file number or a time"),
            ],
        ),
        (
            "Grid",
            &[
                ("g", "Open / close the grid"),
                ("Arrows  Enter", "Move, open"),
                ("Tab  Right click", "Menu for the file"),
                ("Ctrl++  Ctrl+-", "Bigger / smaller pictures"),
                ("i", "Titles under the pictures"),
            ],
        ),
    ],
    &[
        (
            "Video",
            &[
                ("Space", "Play / pause; held down, or the mouse button held on the picture, twice the speed"),
                ("Left  Right", "Seek about 5 seconds, on into the next file or back into the one before"),
                ("PgUp  PgDn", "Seek 1 minute"),
                ("Shift+Left  Shift+Right", "Step one frame"),
                ("b", "Back to the start"),
                ("]  [  \\", "Faster / slower / normal speed"),
                ("Up  Down  m", "Volume, mute"),
                ("l", "Loop: this video, the whole list, off"),
                ("T", "Scenes of this video along the bottom; Left / Right then go by scene"),
                ("j  J", "Next subtitles, or none / next audio track"),
                ("Ctrl+Left  Ctrl+Right", "Previous / next file"),
                ("Pointer at the bottom", "Seek bar with previews"),
                ("At the end", "The next file plays, unless this one loops"),
            ],
        ),
        (
            "Zoom and turn",
            &[
                ("+  -", "Zoom in / out"),
                ("0  *  !", "Fit / actual size / fill"),
                ("Z", "Small images: fit or actual size"),
                ("k", "Keep zoom between files"),
                ("Ctrl+arrows  Alt+arrows", "Scroll, by a page"),
                ("Drag", "Pan"),
                ("Middle drag  Ctrl+wheel", "Zoom"),
                (")  (", "Rotate right / left"),
                ("_  |", "Flip / mirror"),
            ],
        ),
    ],
    &[
        (
            "Views",
            &[
                ("f  Double click", "Fullscreen"),
                ("t", "Filmstrip"),
                ("p", "File list down the side"),
                ("i", "Info: name, size, zoom"),
                ("e", "EXIF data"),
                ("Tab  Right click", "Menu"),
                ("h", "Slideshow"),
                ("W", "Watch for new files"),
                ("r  F5", "Refresh the list"),
            ],
        ),
        (
            "Marks and files",
            &[
                ("x", "Mark / unmark"),
                ("X", "Menu for the marked"),
                ("V  M", "Copy / move to the -o folder"),
                ("c  C", "Copy name / full path"),
                ("F2", "Rename"),
                ("u", "Undo copy, move or rename"),
                ("Delete", "Remove from the list"),
                ("Ctrl+Delete", "Move to trash"),
                ("D", "Delete for good (asks)"),
                ("S  L", "Save a copy / the file list"),
                ("Enter  1-9", "Run an action"),
            ],
        ),
        (
            "Leaving",
            &[("q  Esc", "Quit")],
        ),
    ],
];
