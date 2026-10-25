use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::convert::Format;
use crate::transcode::{Clip, Codec, Container, Target};
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
    /// A speed from `SPEEDS`, by its place there; and the seek bar's
    /// cog, the menu of how a video plays.
    SetSpeed(u8),
    PlaybackSettings,
    /// Mark the start or the end of a section of the video, which then
    /// loops; and unmark it.
    SectionStart,
    SectionEnd,
    SectionClear,
    /// Loop the section, or play on through it and past its end.
    SectionLoop,
    /// Play the section from its start.
    SectionPlay,
    /// The Convert window, for the section.
    SectionConvert,
    /// From the seek bar's own menu, at the time right-clicked: a new
    /// section started there, the one marked ended there, or play from there.
    SectionStartAt,
    SectionEndAt,
    PlayFromHere,
    /// Every section of the video unmarked, not only the one in use.
    SectionClearAll,
    /// Save the section as a clip or an animation.
    SaveSection(Clip),
    /// Keep the section, for the Sections tab; or forget it, kept.
    KeepSection,
    /// Name a kept section: the one right-clicked in the Sections tab, or
    /// the one marked.
    RenameSection,
    /// Remove a kept section from the Sections tab.
    ForgetSection,
    /// Stop the video being converted or saved, and those after it.
    CancelEncode,
    Flip,
    Mirror,
    ToggleAntialias,
    Render,
    ToggleFullscreen,
    /// Out of fullscreen to a small window: the size last given one, or
    /// a quarter of the screen.
    SmallWindow,
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
    /// The picker of a video's audio and subtitle tracks.
    TrackPicker,
    /// What the file is made of: its streams, HDR, tags and chapters.
    Inspect,
    /// Keep, order, tag, add and take out a video's tracks.
    EditTracks,
    /// Draw on the picture, crop it and round its corners, then save it
    /// over itself or as a copy.
    Edit,
    SubtitlesLarger,
    SubtitlesSmaller,
    /// Put the debug overlay's figures on the clipboard, plainly or as JSON.
    CopyDebug,
    CopyDebugJson,
    /// Show one of the graphs of the picture'"'"'s colors under the debug
    /// overlay, or stop; and draw their counts by logarithm or plainly.
    Scope(usize),
    /// One of the checks drawn on the video as it plays, on or off: zebra
    /// stripes over what is clipped, false color for exposure, or what is
    /// out of BT.709 in a wide-gamut video, on the frame paused on.
    Check(usize),
    ScopeLog,
    /// Step an Ultra HDR photo through its renditions: the ordinary
    /// picture, the HDR one, and the gain map that makes it.
    CycleHdr,
    /// Show Ultra HDR photos in this rendition.
    SetHdr(crate::ultrahdr::Hdr),
    /// The headroom, in stops, the HDR curves under the debug overlay are
    /// drawn for: a quarter of a stop up, or down.
    HdrHeadroom(bool),
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
    /// Narrow the list to its pictures, or to its videos.
    OnlyImages,
    OnlyVideos,
    CopyName,
    /// Copy the name of the file on view alone, marks or none: a click on
    /// it in the info overlay.
    CopyViewedName,
    /// The whole path of the file on view: a click on its folder there.
    CopyViewedPath,
    /// Its dimensions, as 1920x1080: a click on them there.
    CopyViewedSize,
    /// The folder of the file on view; and from the seek bar, its title,
    /// the time, one of the details over the track, or all of them.
    CopyViewedFolder,
    CopyBarTitle,
    CopyTime,
    CopyDetail(u8),
    CopyDetails,
    CopyPath,
    /// Copy the paths on one line for a shell, each folder's files folded
    /// together with braces.
    CopyShell,
    CopyTo,
    MoveTo,
    /// The same, into a folder typed there and then.
    CopyToAsk,
    MoveToAsk,
    Undo,
    /// Do again what was last undone.
    Redo,
    /// The Activity panel: conversions and saves, running and done, and
    /// what the toasts said.
    Activity,
    /// Back to where the last jump was made from, and on again.
    TravelBack,
    TravelForward,
    /// Copy one of those.
    CopyMessage(usize),
    Rename,
    Convert(Format),
    /// Open the Convert panel: each format's settings, tried on the
    /// picture before it is converted.
    ConvertPanel,
    /// Convert the target videos by ffmpeg.
    ConvertVideo(Target),
    /// Compare two pictures: the two marked, or the one marked and the one
    /// on view.
    Compare,
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
    /// Keep or reject a file as a triage does, wherever it is asked; and
    /// narrow the list to one of the three lots a triage makes.
    Keep,
    Reject,
    OnlyKept,
    OnlyRejected,
    OnlyUndecided,
    /// Type a shell command to run on the file on view, or on the marked
    /// ones: the find box, set to that.
    Shell,
    /// Run what was typed there, or the command at this place in the list
    /// of those run before, or the last one run once more.
    RunTyped,
    RunShell(usize),
    /// Go back to a file seen lately, by its number: from the find box.
    OpenRecent(usize),
    RepeatShell,
    /// In the grid, go back to the video that is playing.
    ShowPlaying,
    /// Type a search into the heading of the file list.
    ListSearch,
    /// Stand the file list beside the picture, or over it again; and move
    /// it to the other side of the window.
    ListPin,
    ListFlip,
    /// Takes the file list out into a window of its own, or back.
    ListPopOut,
    /// Drop a kept search without going back to where it was begun.
    ShowAllHere,
    /// The go-to box: type part of a name, pick a match, go to it.
    Palette,
    /// The same box in command mode: `>` typed, the commands listed.
    CommandPalette,
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
            // As < and >: the file after, the file before.
            NamedKey::PageDown => Next,
            NamedKey::PageUp => Previous,
            NamedKey::Delete if modifiers.control_key() => Trash,
            NamedKey::Delete => Remove,
            NamedKey::Enter => Action(0),
            NamedKey::Escape => Quit,
            NamedKey::Tab if modifiers.control_key() && modifiers.shift_key() => PrevWorkspace,
            NamedKey::Tab if modifiers.control_key() => NextWorkspace,
            NamedKey::Tab => ToggleMenu,
            NamedKey::F2 => Rename,
            NamedKey::F3 | NamedKey::F12 => Inspect,
            NamedKey::F5 => Reload,
            _ => return None,
        },
        Key::Character(text) => match text {
            ">" | "n" => Next,
            "." => RepeatShell,
            "<" | "," => Previous,
            "p" if modifiers.control_key() || modifiers.super_key() => CommandPalette,
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
            "A" => SectionStart,
            "B" => SectionEnd,
            "R" => Render,
            "f" => ToggleFullscreen,
            "t" => ToggleFilmstrip,
            "D" => Delete,
            "o" if modifiers.control_key() => TravelBack,
            // Inspect, as a browser opens its developer tools.
            "I" | "i" if modifiers.control_key() && modifiers.shift_key() => Inspect,
            "i" if modifiers.control_key() => TravelForward,
            "i" => ToggleInfo,
            "I" => ToggleDebug,
            "e" => Edit,
            "E" => ToggleExif,
            "a" => ToggleActions,
            "o" => TogglePointer,
            "M" => MoveTo,
            "V" => CopyTo,
            "u" => Undo,
            "U" => Redo,
            "x" => Mark,
            "X" => MarkMenu,
            "c" => CopyName,
            "C" => CopyPath,
            "Y" => CopyShell,
            "g" => ToggleGrid,
            "G" => Overview,
            "?" => ToggleHelp,
            "h" => ToggleSlideshow,
            "w" => SizeToImage,
            "r" => Reload,
            "S" => Save,
            "K" => ConvertPanel,
            "N" => Activity,
            "s" => SortMenu,
            "m" => ToggleMute,
            "l" => CycleLoop,
            "T" => ToggleScenes,
            "H" => CycleHdr,
            "j" if modifiers.control_key() => TrackPicker,
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
    /// Only while the list holds both pictures and videos.
    Mixed,
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

const fn mixed_item(label: &'static str, keys: &'static str, command: Command) -> Option<MenuItem> {
    Some(MenuItem { label, keys, command, applies: Applies::Mixed, sub: &[] })
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
    /// A tick and a cross, each white on a filled circle.
    pub const CHECK_FILLED: &str = "\u{f001}";
    pub const X_FILLED: &str = "\u{f002}";
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
    /// The pop-out arrow turned back: into the main window again.
    pub const POP_IN: &str = "\u{e600}";
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
    // Edit's tools.
    pub const SELECT: &str = "\u{e700}";
    pub const RECT: &str = "\u{e701}";
    pub const OVAL: &str = "\u{e702}";
    pub const ARROW_UP_RIGHT: &str = "\u{e703}";
    pub const LINE: &str = "\u{e704}";
    pub const MARKER: &str = "\u{e705}";
    pub const COUNTER: &str = "\u{e706}";
    pub const BLUR: &str = "\u{e707}";
    pub const SPOTLIGHT: &str = "\u{e708}";
    pub const CROP: &str = "\u{e709}";
    pub const EDIT: &str = "\u{e70a}";
    pub const REDO: &str = "\u{e70b}";
    pub const STICKER: &str = "\u{e70c}";
    pub const PIPETTE: &str = "\u{e70d}";
    pub const BADGE: &str = "\u{e70e}";
    pub const ERASER: &str = "\u{e70f}";
    pub const TEXT_SCAN: &str = "\u{e710}";
    pub const REDACT: &str = "\u{e711}";
    pub const PIP: &str = "\u{e712}";
    pub const SETTINGS: &str = "\u{e713}";
}

/// The icon drawn beside a menu item.
pub fn icon(item: &MenuItem) -> &'static str {
    use icons::*;
    if !item.sub.is_empty() {
        return match item.label {
            "Playback" => MONITOR_PLAY,
            "Section" => REPEAT,
            "Zoom" => ZOOM_IN,
            "Rotate" => ROTATE_CW,
            "View" => EYE,
            "Sort" => SORT,
            "Marks" => STAR,
            "File" => FILE,
            "Debug" => BUG,
            "Triage" => FLAG,
            "Go" => CHEVRONS_RIGHT,
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
        Search | Palette | CommandPalette => SEARCH,
        WorkspaceList | Overview | Workspace(_) | WorkspaceKey(_) | NextWorkspace | PrevWorkspace | HomeWorkspace => FOLDERS,
        SaveWorkspace => FOLDER_PLUS,
        ClearFilter | ShowAllHere => LIST_X,
        Triage => FLAG,
        Keep => CHECK_FILLED,
        Reject => X_FILLED,
        OnlyKept | OnlyRejected | OnlyUndecided => LIST_FILTER,
        Shell | RunTyped | RunShell(_) | RepeatShell => TERMINAL,
        OpenRecent(_) => CLOCK,
        ListSearch => SEARCH,
        ListPin => PIN,
        ListFlip => REPLACE,
        ListPopOut => EXTERNAL_LINK,
        ClearMarks => STAR_OFF,
        Shuffle => SHUFFLE,
        PlayPause => PLAY,
        ShowPlaying => LAYOUT_GRID,
        SeekForward => FAST_FORWARD,
        SeekBack => REWIND,
        FrameForward => STEP_FORWARD,
        FrameBack => STEP_BACK,
        Restart => SKIP_BACK,
        SectionStart | SectionEnd => REPEAT,
        SectionClear => X,
        SectionLoop => REPEAT,
        SectionPlay => PLAY,
        SectionConvert => REPLACE,
        SectionStartAt | SectionEndAt => REPEAT,
        PlayFromHere => PLAY,
        SectionClearAll => X,
        SaveSection(_) => FILM,
        KeepSection => PIN,
        RenameSection => PENCIL,
        ForgetSection | CancelEncode => X,
        SpeedUp => CHEVRONS_RIGHT,
        SpeedDown => CHEVRONS_LEFT,
        SpeedReset => GAUGE,
        // A list of speeds: the one playing ticked, the rest plain.
        SetSpeed(_) => "",
        PlaybackSettings => SETTINGS,
        VolumeUp => VOLUME_2,
        VolumeDown => VOLUME_1,
        ToggleMute => VOLUME_X,
        CycleLoop => REPEAT,
        TogglePrecise => SCAN,
        ToggleScenes => CLAPPERBOARD,
        CycleSubtitles => CAPTIONS,
        CycleAudio | TrackPicker => AUDIO_LINES,
        Inspect => EYE,
        EditTracks => AUDIO_LINES,
        Edit => EDIT,
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
        SmallWindow => PIP,
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
        OnlyImages => IMAGES,
        OnlyVideos => FILM,
        CopyTo | CopyToAsk => FOLDER_PLUS,
        MoveTo | MoveToAsk => FOLDER_INPUT,
        Rename => PENCIL,
        Convert(_) | ConvertPanel | ConvertVideo(_) => REPLACE,
        Compare => EYE,
        Undo => UNDO,
        Redo => ROTATE_CW,
        CopyMessage(_) => CLOCK,
        Activity => LIST,
        TravelBack => ARROW_LEFT,
        TravelForward => ARROW_RIGHT,
        CopyName | CopyViewedName | CopyViewedPath | CopyViewedSize | SaveFilelist => COPY,
        CopyViewedFolder => FOLDERS,
        CopyBarTitle | CopyTime | CopyDetail(_) | CopyDetails => COPY,
        CopyPath | CopyShell => CLIPBOARD,
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
    item("Find…", "/", Command::Search),
    mixed_item("Only pictures", "", Command::OnlyImages),
    mixed_item("Only videos", "", Command::OnlyVideos),
    Some(MenuItem { label: "Back from this search", keys: "Esc", command: Command::ClearFilter, applies: Applies::Filtered, sub: &[] }),
    Some(MenuItem { label: "Show all files, stay on this one", keys: "", command: Command::ShowAllHere, applies: Applies::Filtered, sub: &[] }),
    item("Shuffle", "z", Command::Shuffle),
    item("Workspaces", "G", Command::Overview),
    item("Go to workspace…", "@", Command::WorkspaceList),
    image_item("Edit", "e", Command::Edit),
    // A window of its own for a picture; the formats, for a video.
    item("Convert…", "K", Command::ConvertPanel),
    submenu("Convert to", CONVERT_PICTURE, Applies::Images),
    item("Run a command on it…", "!", Command::Shell),
    item("Repeat last command", ".", Command::RepeatShell),
    submenu("Triage", TRIAGE, Applies::Always),
    None,
    submenu("Go", GO, Applies::Always),
    submenu("Playback", PLAYBACK, Applies::Videos),
    submenu("Section", SECTION, Applies::Videos),
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
    None,
    // Last, as a browser has it.
    item("Inspect", "Ctrl+Shift+I", Command::Inspect),
];

/// A picture converted at once, as one of these, with the settings last
/// used; the Convert window tries them first.
const CONVERT_PICTURE: &[Option<MenuItem>] = &[
    image_item("JPEG XL", "", Command::Convert(Format::Jxl)),
    image_item("AVIF", "", Command::Convert(Format::Avif)),
    image_item("JPEG", "", Command::Convert(Format::Jpeg)),
    image_item("PNG", "", Command::Convert(Format::Png)),
    image_item("WebP", "", Command::Convert(Format::WebP)),
];

/// A stretch of a video marked between A and B: kept, saved, named.
const SECTION: &[Option<MenuItem>] = &[
    video_item("Start here", "Shift+A", Command::SectionStart),
    video_item("End here", "Shift+B", Command::SectionEnd),
    video_item("Clear", "Esc", Command::SectionClear),
    video_item("Clear all", "", Command::SectionClearAll),
    video_item("Loop / play through", "", Command::SectionLoop),
    None,
    video_item("Keep / forget", "", Command::KeepSection),
    video_item("Rename kept", "", Command::RenameSection),
    video_item("Remove kept", "", Command::ForgetSection),
    None,
    video_item("Save quick", "", Command::SaveSection(Clip::Quick)),
    video_item("Save MP4  ·  H.264", "", Command::SaveSection(Clip::Exact(Target::new(Codec::H264, Container::Mp4)))),
    video_item("Save MP4  ·  HEVC", "", Command::SaveSection(Clip::Exact(Target::new(Codec::Hevc, Container::Mp4)))),
    video_item("Save MKV  ·  AV1", "", Command::SaveSection(Clip::Exact(Target::new(Codec::Av1, Container::Mkv)))),
    video_item("Save GIF", "", Command::SaveSection(Clip::Gif)),
    video_item("Save WebP", "", Command::SaveSection(Clip::WebP)),
    video_item("Save AVIF", "", Command::SaveSection(Clip::Avif)),
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
    video_item("Loop  ·  off, video, list", "l", Command::CycleLoop),
    video_item("Scenes", "T", Command::ToggleScenes),
    video_item("Precise seeking", "", Command::TogglePrecise),
    None,
    video_item("Next subtitles", "j", Command::CycleSubtitles),
    video_item("Larger subtitles", "", Command::SubtitlesLarger),
    video_item("Smaller subtitles", "", Command::SubtitlesSmaller),
    video_item("Next audio track", "J", Command::CycleAudio),
    video_item("Audio and subtitles…", "Ctrl+J", Command::TrackPicker),
    video_item("Edit tracks…", "", Command::EditTracks),
];

/// Getting about the list further than one file at a time.
const GO: &[Option<MenuItem>] = &[
    item("First file", "Home", Command::First),
    item("Last file", "End", Command::Last),
    item("Forward 5% of the files", "", Command::JumpForward),
    item("Back 5% of the files", "", Command::JumpBack),
    item("Back from the jump", "Ctrl+O", Command::TravelBack),
    item("Forward again", "Ctrl+I", Command::TravelForward),
    item("Next folder", "}", Command::NextDir),
    item("Previous folder", "{", Command::PreviousDir),
    None,
    item("Find a file…", "Ctrl+K", Command::Palette),
    item("Run a command…", "Ctrl+P", Command::CommandPalette),
    item("Next workspace", "Ctrl+Tab", Command::NextWorkspace),
    item("Previous workspace", "Ctrl+Shift+Tab", Command::PrevWorkspace),
    item("Back to the start folder", "Ctrl+`", Command::HomeWorkspace),
];

const ZOOM: &[Option<MenuItem>] = &[
    item("Zoom in", "+", Command::ZoomIn),
    item("Zoom out", "-", Command::ZoomOut),
    item("Actual size", "*", Command::ZoomActual),
    item("Fit to screen", "0", Command::ZoomFit),
    item("Fill screen", "F", Command::ZoomFill),
    item("Fit small images to screen", "Z", Command::ToggleEnlarge),
    item("Keep zoom between files", "k", Command::ToggleKeepView),
    item("Smooth scaling", "", Command::ToggleAntialias),
    item("Full-quality redraw", "R", Command::Render),
];

const ROTATE: &[Option<MenuItem>] = &[
    item("Rotate right", ")", Command::RotateRight),
    item("Rotate left", "(", Command::RotateLeft),
    item("Flip vertically", "_", Command::Flip),
    item("Mirror horizontally", "|", Command::Mirror),
];

const VIEW: &[Option<MenuItem>] = &[
    item("Fullscreen", "f", Command::ToggleFullscreen),
    item("Small window", "", Command::SmallWindow),
    item("Grid view", "g", Command::ToggleGrid),
    item("Filmstrip", "t", Command::ToggleFilmstrip),
    item("File list", "p", Command::ToggleList),
    item("File list beside the picture", "", Command::ListPin),
    item("File list on the other side", "", Command::ListFlip),
    item("Search the file list", "/", Command::ListSearch),
    item("Slideshow", "h", Command::ToggleSlideshow),
    item("Watch for new files", "W", Command::ToggleWatch),
    None,
    item("Info", "i", Command::ToggleInfo),
    image_item("EXIF data", "E", Command::ToggleExif),
    image_item("Ultra HDR  ·  SDR, HDR, gain map", "H", Command::CycleHdr),
    item("List of actions", "a", Command::ToggleActions),
    None,
    item("Fit window to picture", "w", Command::SizeToImage),
    item("Hide the pointer", "o", Command::TogglePointer),
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

/// Deciding about files, and what can be done with what was decided.
const TRIAGE: &[Option<MenuItem>] = &[
    item("Triage, file by file", "Q", Command::Triage),
    item("Keep", "Space", Command::Keep),
    item("Reject", "x", Command::Reject),
    None,
    item("Show only kept", "", Command::OnlyKept),
    item("Show only rejected", "", Command::OnlyRejected),
    item("Show only undecided", "", Command::OnlyUndecided),
    None,
    item("Run a command on a lot…", "!", Command::Shell),
    item("Look for new files", "r", Command::Reload),
];

const MARKS: &[Option<MenuItem>] = &[
    item("Mark", "x", Command::Mark),
    item("Marked images…", "X", Command::MarkMenu),
    item("Mark all", "", Command::MarkAll),
    item("Invert marks", "", Command::InvertMarks),
    item("Show only marked", "", Command::OnlyMarked),
    item("Clear all marks", "", Command::ClearMarks),
];

const FILE: &[Option<MenuItem>] = &[
    item("Copy to folder", "V", Command::CopyTo),
    item("Move to folder", "M", Command::MoveTo),
    item("Copy to…", "", Command::CopyToAsk),
    item("Move to…", "", Command::MoveToAsk),
    item("Rename", "F2", Command::Rename),
    item("Cancel the encode", "", Command::CancelEncode),
    image_item("Compare with marked", "", Command::Compare),
    item("Inspect metadata", "F3", Command::Inspect),
    image_item("Save a copy", "S", Command::Save),
    video_item("Save this frame", "S", Command::Save),
    item("Save the file list", "L", Command::SaveFilelist),
    item("Undo", "u", Command::Undo),
    item("Redo", "U", Command::Redo),
    item("Activity", "N", Command::Activity),
    None,
    item("Copy file name", "c", Command::CopyName),
    item("Copy full path", "C", Command::CopyPath),
    item("Copy for the shell", "Y", Command::CopyShell),
    None,
    item("Refresh", "r", Command::Reload),
    item("Remove from list", "Del", Command::Remove),
    item("Move to trash", "Ctrl+Del", Command::Trash),
    item("Delete…", "D", Command::Delete),
    None,
    item("Save folder as workspace…", "", Command::SaveWorkspace),
];

const DEBUG: &[Option<MenuItem>] = &[
    item("Timings on screen", "I", Command::ToggleDebug),
    item("Print files to terminal", "", Command::ToggleVerbose),
    item("Print timings to terminal", "", Command::ToggleDebugLog),
    item("Copy the timings", "", Command::CopyDebug),
    item("Copy the timings as JSON", "", Command::CopyDebugJson),
    None,
    item("Graph: histogram", "", Command::Scope(0)),
    item("Graph: histogram in rows", "", Command::Scope(1)),
    item("Graph: waveform", "", Command::Scope(2)),
    item("Graph: RGB parade", "", Command::Scope(3)),
    item("Graph: vectorscope", "", Command::Scope(4)),
    item("Graphs by logarithm", "", Command::ScopeLog),
    item("Check: zebra, clipped highlights", "", Command::Check(0)),
    item("Check: false color, exposure", "", Command::Check(1)),
    item("Check: out of BT.709, paused", "", Command::Check(2)),
    item("HDR curves: more headroom", "", Command::HdrHeadroom(true)),
    item("HDR curves: less headroom", "", Command::HdrHeadroom(false)),
];

/// The menu for a file in the grid, on a right click or Tab.
pub const GRID_MENU: &[Option<MenuItem>] = &[
    video_item("Play", "Enter", Command::PlayFromGrid),
    image_item("Open", "Enter", Command::OpenFromGrid),
    item("Mark", "x", Command::Mark),
    item("Keep", "", Command::Keep),
    item("Reject", "", Command::Reject),
    None,
    item("Copy to folder", "V", Command::CopyTo),
    item("Move to folder", "M", Command::MoveTo),
    item("Copy to…", "", Command::CopyToAsk),
    item("Move to…", "", Command::MoveToAsk),
    item("Rename", "F2", Command::Rename),
    item("Convert…", "K", Command::ConvertPanel),
    submenu("Convert to", CONVERT_PICTURE, Applies::Images),
    None,
    item("Copy file name", "c", Command::CopyName),
    item("Copy full path", "C", Command::CopyPath),
    item("Copy for the shell", "Y", Command::CopyShell),
    item("Run a command on it…", "!", Command::Shell),
    None,
    item("Remove from list", "Del", Command::Remove),
    item("Move to trash", "Ctrl+Del", Command::Trash),
    item("Delete…", "D", Command::Delete),
    None,
    item("Find…", "/", Command::Search),
    mixed_item("Only pictures", "", Command::OnlyImages),
    mixed_item("Only videos", "", Command::OnlyVideos),
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
    item("Keep", "", Command::Keep),
    item("Reject", "", Command::Reject),
    None,
    item("Copy to folder", "V", Command::CopyTo),
    item("Move to folder", "M", Command::MoveTo),
    item("Copy to…", "", Command::CopyToAsk),
    item("Move to…", "", Command::MoveToAsk),
    item("Rename", "F2", Command::Rename),
    item("Convert…", "K", Command::ConvertPanel),
    submenu("Convert to", CONVERT_PICTURE, Applies::Images),
    None,
    item("Copy file name", "c", Command::CopyName),
    item("Copy full path", "C", Command::CopyPath),
    item("Copy for the shell", "Y", Command::CopyShell),
    item("Run a command on it…", "!", Command::Shell),
    None,
    item("Remove from list", "Del", Command::Remove),
    item("Move to trash", "Ctrl+Del", Command::Trash),
    item("Delete…", "D", Command::Delete),
    None,
    item("Find…", "/", Command::Search),
    mixed_item("Only pictures", "", Command::OnlyImages),
    mixed_item("Only videos", "", Command::OnlyVideos),
    submenu("Sort", SORT, Applies::Always),
    submenu("Marks", MARKS, Applies::Always),
];

/// The menu for the grid as a whole, on a right click off the pictures.
pub const GRID_AROUND_MENU: &[Option<MenuItem>] = &[
    item("Find…", "/", Command::Search),
    mixed_item("Only pictures", "", Command::OnlyImages),
    mixed_item("Only videos", "", Command::OnlyVideos),
    submenu("Sort", SORT, Applies::Always),
    item("Shuffle", "z", Command::Shuffle),
    item("Workspaces", "G", Command::Overview),
    item("Go to workspace…", "@", Command::WorkspaceList),
    item("Run a command on it…", "!", Command::Shell),
    item("Repeat last command", ".", Command::RepeatShell),
    submenu("Triage", TRIAGE, Applies::Always),
    item("Save folder as workspace…", "", Command::SaveWorkspace),
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

/// A right click on the section marked, on the seek bar.
pub const SECTION_BAR_MENU: &[Option<MenuItem>] = &[
    video_item("Play from the start", "", Command::SectionPlay),
    video_item("Convert…", "", Command::SectionConvert),
    None,
    video_item("Loop / play through", "", Command::SectionLoop),
    video_item("Keep / forget", "", Command::KeepSection),
    video_item("Clear", "Esc", Command::SectionClear),
    None,
    video_item("Save quick", "", Command::SaveSection(Clip::Quick)),
    video_item("Save MP4  ·  H.264", "", Command::SaveSection(Clip::Exact(Target::new(Codec::H264, Container::Mp4)))),
    video_item("Save MP4  ·  HEVC", "", Command::SaveSection(Clip::Exact(Target::new(Codec::Hevc, Container::Mp4)))),
    video_item("Save MKV  ·  AV1", "", Command::SaveSection(Clip::Exact(Target::new(Codec::Av1, Container::Mkv)))),
    video_item("Save GIF", "", Command::SaveSection(Clip::Gif)),
    video_item("Save WebP", "", Command::SaveSection(Clip::WebP)),
    video_item("Save AVIF", "", Command::SaveSection(Clip::Avif)),
];

/// The seek bar's title, time or details right-clicked: what of the file to copy.
pub const BAR_INFO_MENU: &[Option<MenuItem>] = &[
    video_item("Copy file name", "", Command::CopyViewedName),
    video_item("Copy full path", "", Command::CopyViewedPath),
    video_item("Copy folder", "", Command::CopyViewedFolder),
    None,
    video_item("Copy title", "", Command::CopyBarTitle),
    video_item("Copy time", "", Command::CopyTime),
    video_item("Copy all details", "", Command::CopyDetails),
];

/// The speeds, each by its place in `SPEEDS`, the one playing ticked.
const SPEED_MENU: &[Option<MenuItem>] = &[
    video_item("0.25×", "", Command::SetSpeed(0)),
    video_item("0.5×", "", Command::SetSpeed(1)),
    video_item("0.75×", "", Command::SetSpeed(2)),
    video_item("Normal", "\\", Command::SetSpeed(3)),
    video_item("1.25×", "", Command::SetSpeed(4)),
    video_item("1.5×", "", Command::SetSpeed(5)),
    video_item("2×", "", Command::SetSpeed(6)),
    video_item("3×", "", Command::SetSpeed(7)),
    video_item("4×", "", Command::SetSpeed(8)),
];

/// The seek bar's cog: how the video plays.
pub const PLAYBACK_MENU: &[Option<MenuItem>] = &[
    submenu("Speed", SPEED_MENU, Applies::Videos),
    video_item("Audio and subtitles…", "Ctrl+J", Command::TrackPicker),
];

/// The seek bar right-clicked away from a section: at the time clicked.
pub const SEEK_BAR_MENU: &[Option<MenuItem>] = &[
    video_item("Play from here", "", Command::PlayFromHere),
    None,
    video_item("Start a section here", "", Command::SectionStartAt),
    video_item("End the section here", "", Command::SectionEndAt),
    None,
    video_item("Clear all sections", "", Command::SectionClearAll),
];

/// The section's save button's other half: what to save it as.
pub const SECTION_SAVE_MENU: &[Option<MenuItem>] = &[
    video_item("Quick  ·  as it is", "", Command::SaveSection(Clip::Quick)),
    video_item("MP4  ·  H.264", "", Command::SaveSection(Clip::Exact(Target::new(Codec::H264, Container::Mp4)))),
    video_item("MP4  ·  HEVC", "", Command::SaveSection(Clip::Exact(Target::new(Codec::Hevc, Container::Mp4)))),
    video_item("MKV  ·  AV1", "", Command::SaveSection(Clip::Exact(Target::new(Codec::Av1, Container::Mkv)))),
    video_item("GIF", "", Command::SaveSection(Clip::Gif)),
    video_item("WebP", "", Command::SaveSection(Clip::WebP)),
    video_item("AVIF", "", Command::SaveSection(Clip::Avif)),
];

/// A right click on a kept section in the Sections tab.
pub const SECTION_MENU: &[Option<MenuItem>] = &[
    item("Rename", "", Command::RenameSection),
    item("Remove", "", Command::ForgetSection),
];

/// The `X` menu: what can be done with the marked images.
pub const MARK_MENU: &[Option<MenuItem>] = &[
    item("Copy to folder", "V", Command::CopyTo),
    item("Move to folder", "M", Command::MoveTo),
    item("Copy to…", "", Command::CopyToAsk),
    item("Move to…", "", Command::MoveToAsk),
    None,
    item("Copy file names", "c", Command::CopyName),
    item("Copy full paths", "C", Command::CopyPath),
    item("Copy for the shell", "Y", Command::CopyShell),
    item("Save list to a file", "L", Command::SaveFilelist),
    None,
    item("Convert…", "K", Command::ConvertPanel),
    submenu("Convert to", CONVERT_PICTURE, Applies::Images),
    image_item("Compare the two", "", Command::Compare),
    None,
    item("Move to trash", "Ctrl+Del", Command::Trash),
    item("Delete…", "D", Command::Delete),
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
    ("Copy for the shell  Y", Command::CopyShell),
    ("Delete  D", Command::Delete),
    ("More  X", Command::MarkMenu),
];

/// The few keys at the head of the `?` overlay, large, before the whole
/// list: what most of jog is used with.
pub const ESSENTIALS: &[(&str, &str)] = &[
    ("Right  Left", "Next / previous"),
    ("Space", "Play / pause"),
    ("f", "Fullscreen"),
    ("g", "Grid"),
    ("p", "File list"),
    ("Ctrl+K", "Find a file"),
    ("Ctrl+P", "Run a command"),
    ("i", "Info"),
    ("K", "Convert"),
    ("Tab", "Menu"),
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
                ("PgDn  PgUp", "Next / previous file, as > and <"),
                ("}  {", "Next / previous folder"),
                ("Wheel", "Next / previous file"),
                ("z", "Shuffle: to another file"),
                ("!  .", "Shell command on it (%f its path) / again"),
                ("s", "Sort: name, newest, size, reverse"),
            ],
        ),
        (
            "Finding",
            &[
                ("/  Ctrl+K", "Find a file"),
                ("Ctrl+P", "Run a command"),
                ("Enter", "Go to the match picked"),
                ("Tab  Enter", "To the buttons: show only the matches"),
                ("Esc", "Leave a search, back to where it was begun"),
                ("G", "Workspaces: look over them all"),
                ("@  Ctrl+1  Ctrl+Tab", "Workspaces: list, go to, next"),
                ("Ctrl+`", "Back to what jog was started on"),
                ("word  ^start  !not", "Match names"),
                ("#image  #video  #marked  >10mb", "Match kinds and sizes"),
                ("is:new  is:kept  is:rejected", "Unwatched, or as triaged"),
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
                ("Space", "Pause the video still playing in its cell"),
                ("Click its picture", "Back to the video playing"),
            ],
        ),
    ],
    &[
        (
            "Video",
            &[
                ("Space", "Play / pause; held, twice the speed"),
                ("Left  Right", "Seek 5 s, on into the next file"),
                ("PgUp  PgDn", "Previous / next file, as < and >"),
                ("Shift+Left  Shift+Right", "Step one frame"),
                ("b", "Back to the start"),
                ("Shift+A  Shift+B", "Section start / end: loops; Esc clears"),
                ("Right click on the bar", "Sections: start, end, more than one"),
                ("]  [  \\", "Faster / slower / normal speed"),
                ("Up  Down  m", "Volume, mute"),
                ("l", "Loop: this video, the whole list, off"),
                ("T", "Scenes along the bottom"),
                ("S", "Save the frame on view as a picture"),
                ("j  J", "Next subtitles, or none / next audio track"),
                ("Ctrl+J", "Pick audio and subtitles; e edits the tracks"),
                ("F3", "Inspect metadata: streams, HDR, structure, EXIF"),
                ("Ctrl+Left  Ctrl+Right", "Previous / next file"),
                ("Pointer at the bottom", "Seek bar with previews"),
                ("At the end", "The next file plays, unless this one loops"),
            ],
        ),
        (
            "Zoom and turn",
            &[
                ("+  -", "Zoom in / out"),
                ("0  *  F", "Fit / actual size / fill"),
                ("v  Wheel", "Loupe while held; wheel, its strength"),
                ("Z", "Small images: fit or actual size"),
                ("H", "Ultra HDR: SDR, HDR, gain map"),
                ("k", "Keep zoom between files"),
                ("R", "Draw at full quality now"),
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
                ("p", "File list; / searches it"),
                ("i", "Info: name, size, zoom"),
                ("I", "Timings, streams, color graphs, HDR curves"),
                ("E", "EXIF data"),
                ("a", "List of the actions there are"),
                ("w  o", "Size the window to the picture / hide the pointer"),
                ("Tab  Right click", "Menu"),
                ("h", "Slideshow"),
                ("W", "Watch for new files"),
                ("r  F5", "Refresh the list"),
            ],
        ),
        (
            "Triage",
            &[
                ("Q", "Begin, and finish: nothing is done to the files"),
                ("Space  x", "Keep / reject, then on; again undoes"),
                ("Left  Right", "Move as usual, to look or to decide again"),
                ("Click the tally", "Look for files that have come or gone"),
                ("!  Tab", "Run a command on the kept, the rejected or all"),
            ],
        ),
        (
            "Marks and files",
            &[
                ("x", "Mark / unmark"),
                ("X", "Menu for the marked"),
                ("V  M", "Copy / move to the -o folder"),
                ("c  C", "Copy name / full path"),
                ("Y", "Copy for the shell: /dir/img-{1,5}.jpg"),
                ("F2", "Rename, or move: a path such as /tmp/a.png"),
                ("K", "Convert: sizes first, a frame to compare"),
                ("N", "Activity: conversions and messages"),
                ("e", "Edit: draw, blur, crop, redact…; Ctrl+S saves"),
                ("u", "Undo copy, move, rename or an edit saved over"),
                ("Delete", "Remove from the list"),
                ("Ctrl+Delete", "Move to trash"),
                ("D", "Delete for good (asks)"),
                ("S  L", "Save a copy, or of a video the frame / the file list"),
                ("Enter  1-9", "Run an action"),
            ],
        ),
        (
            "Leaving",
            &[("?", "These keys"), ("q  Esc", "Quit; jog --resume picks up where it was left")],
        ),
    ],
];
