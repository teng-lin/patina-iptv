//! Shared control icons: our own simple drawings on a 24-unit grid in the
//! spirit of Material's (filled media shapes, 2-unit strokes), colored by
//! the text color.

use nsh_core::element::ImageSrc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Volume,
    Listen,
    Muted,
    Rewind,
    Play,
    Pause,
    Record,
    Clock,
    Sort,
    Check,
    Warning,
    Trash,
    Stop,
    Forward,
    Star,
    StarFilled,
    Subtitles,
    Info,
    PictureSize,
    ChannelList,
    Multiview,
    PictureInPicture,
    Guide,
    Archive,
    FullScreen,
    ExitFullScreen,
    /// A cross (multiview's remove).
    Close,
    /// A window with an arrow into it (picture in picture back to the app).
    BackToApp,
    /// Three dots in a row (a menu's visible way in for touch and pointer).
    More,
    /// A chevron up (the next channel, as Ch+).
    ChannelNext,
    /// A chevron down (the previous channel, as Ch−).
    ChannelPrevious,
}

/// Strokes and fills, as SVG markup.
fn body(icon: Icon) -> &'static str {
    match icon {
        Icon::Volume => {
            "<path fill='currentColor' stroke='none' d='M3 9h4l5-4v14l-5-4H3z'/>\
             <path d='M15.5 8.5a5 5 0 0 1 0 7M18.5 5.5a9 9 0 0 1 0 13'/>"
        }
        // A player window with a waveform identifies the audio source.
        Icon::Listen => {
            "<rect x='2' y='4' width='20' height='16' rx='2'/><path d='M5 12h3l2-4 3 8 2-4h4'/>"
        }
        Icon::Muted => {
            "<path fill='currentColor' stroke='none' d='M3 9h4l5-4v14l-5-4H3z'/>\
             <path d='M16 9l6 6M22 9l-6 6'/>"
        }
        Icon::Rewind => {
            "<path fill='currentColor' stroke='none' d='M11.5 6v12L3 12zM21 6v12l-8.5-6z'/>"
        }
        Icon::Play => "<path fill='currentColor' stroke='none' d='M7 4.5v15L19.5 12z'/>",
        Icon::Record => "<circle cx='12' cy='12' r='6' fill='currentColor' stroke='none'/>",
        Icon::Sort => "<path d='M4 6h16M4 12h11M4 18h6'/>",
        Icon::Clock => "<circle cx='12' cy='12' r='9'/><path d='M12 6v6l4 2'/>",
        Icon::Check => "<path d='M4 12l5 5L20 6'/>",
        Icon::Warning => {
            "<path d='M12 3L2 21h20zM12 9v5'/><circle cx='12' cy='17.5' r='1' fill='currentColor'/>"
        }
        Icon::Trash => "<path d='M4 6h16M9 6V3h6v3M6 6l1 15h10l1-15M10 10v7M14 10v7'/>",
        Icon::Stop => {
            "<rect x='5' y='5' width='14' height='14' rx='1' fill='currentColor' stroke='none'/>"
        }
        Icon::Pause => {
            "<path fill='currentColor' stroke='none' d='M6 4.5h4v15H6zM14 4.5h4v15h-4z'/>"
        }
        Icon::Forward => {
            "<path fill='currentColor' stroke='none' d='M12.5 6v12L21 12zM3 6v12l8.5-6z'/>"
        }
        Icon::Star => {
            "<path d='M12 3.5l2.6 5.4 5.9.8-4.3 4.1 1 5.8L12 16.8l-5.2 2.8 1-5.8-4.3-4.1 5.9-.8z'/>"
        }
        Icon::StarFilled => {
            "<path fill='currentColor' d='M12 3.5l2.6 5.4 5.9.8-4.3 4.1 1 5.8L12 16.8l-5.2 2.8 1-5.8-4.3-4.1 5.9-.8z'/>"
        }
        Icon::Subtitles => {
            "<rect x='3' y='5' width='18' height='14' rx='2'/>\
             <path d='M7 12h3M12 12h5M7 15.5h7M16 15.5h1'/>"
        }
        Icon::Info => {
            "<circle cx='12' cy='12' r='9'/>\
             <path d='M12 11v6'/><circle fill='currentColor' stroke='none' cx='12' cy='7.5' r='1.3'/>"
        }
        Icon::PictureSize => {
            "<rect x='2.5' y='5' width='19' height='14' rx='2'/>\
             <path d='M6 11V8.5h3M18 13v2.5h-3'/>"
        }
        Icon::ChannelList => {
            "<path d='M8 6.5h13M8 12h13M8 17.5h13'/>\
             <circle fill='currentColor' stroke='none' cx='4' cy='6.5' r='1.3'/>\
             <circle fill='currentColor' stroke='none' cx='4' cy='12' r='1.3'/>\
             <circle fill='currentColor' stroke='none' cx='4' cy='17.5' r='1.3'/>"
        }
        Icon::Multiview => {
            "<rect x='3' y='4' width='8' height='7' rx='1.5'/><rect x='13' y='4' width='8' height='7' rx='1.5'/>\
             <rect x='3' y='13' width='8' height='7' rx='1.5'/><rect x='13' y='13' width='8' height='7' rx='1.5'/>"
        }
        Icon::PictureInPicture => {
            "<rect x='2' y='4' width='20' height='16' rx='2'/><rect x='12' y='11' width='8' height='7' rx='1' fill='currentColor'/>"
        }
        Icon::Guide => {
            "<rect x='3' y='4' width='18' height='16' rx='2'/>\
             <path d='M3 9h18M3 14.5h18M9 9v11M15 14.5V20'/>"
        }
        Icon::Archive => "<path d='M3 4h18v5H3zM5 9v11h14V9m-9 5h4'/>",
        Icon::FullScreen => "<path d='M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5'/>",
        Icon::ExitFullScreen => "<path d='M9 4v5H4M20 9h-5V4M15 20v-5h5M4 15h5v5'/>",
        Icon::Close => "<path d='M6 6l12 12M18 6L6 18'/>",
        Icon::BackToApp => {
            "<rect x='2' y='4' width='20' height='16' rx='2'/><path d='M16 15L9 8M9 13V8h5'/>"
        }
        Icon::More => {
            "<circle fill='currentColor' stroke='none' cx='5' cy='12' r='2'/>\
             <circle fill='currentColor' stroke='none' cx='12' cy='12' r='2'/>\
             <circle fill='currentColor' stroke='none' cx='19' cy='12' r='2'/>"
        }
        Icon::ChannelNext => "<path d='M6 15l6-6 6 6'/>",
        Icon::ChannelPrevious => "<path d='M6 9l6 6 6-6'/>",
    }
}

pub fn src(icon: Icon) -> ImageSrc {
    ImageSrc::svg_markup(&format!(
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' width='24' height='24' \
         fill='none' stroke='currentColor' stroke-width='2' stroke-linecap='round' \
         stroke-linejoin='round'>{}</svg>",
        body(icon)
    ))
}
