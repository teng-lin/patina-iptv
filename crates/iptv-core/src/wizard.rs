//! The first-run and add-playlist wizard as a pure state machine.
//!
//! The app feeds [`WizardEvent`]s (remote keys, touch taps, text-entry and
//! clipboard results, import progress and results) into [`Wizard::handle`]
//! and performs the [`WizardEffect`] it returns (open a text entry, read the
//! clipboard, start or cancel an import, close the wizard). Nothing here
//! does I/O or reads a clock.
//!
//! First run opens on welcome with an add action and, where supported, a
//! restore action. The form starts with the M3U link source and offers a
//! source picker for other formats.
//!
//! - A recoverable failure returns to the form with the chosen type and
//!   every entered value kept, focus on the field most likely at fault
//!   (AC02). Provider failures are typed so they never look like an empty
//!   playlist (AC11).
//! - Back from the source picker returns to the form's source control.
//!   Back from the form returns to welcome or closes the settings wizard.
//! - Submitting while an import runs does nothing; results for a canceled
//!   or superseded import are ignored by [`ImportId`].
//!
//! Secrets: usernames, passwords and MAC addresses print as `***` in
//! `Debug`; URLs print only `scheme://host[:port]` because playlist URLs
//! often carry credentials in their path or query. Labels and the empty
//! state's wording are the shell's own (i18n), not stored here.

use std::collections::BTreeMap;
use std::fmt;

/// What the user is adding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PlaylistKind {
    M3uUrl,
    M3uFile,
    Xtream,
    Stalker,
}

impl PlaylistKind {
    /// Type-list order.
    pub const ALL: [PlaylistKind; 4] = [
        PlaylistKind::M3uUrl,
        PlaylistKind::M3uFile,
        PlaylistKind::Xtream,
        PlaylistKind::Stalker,
    ];

    /// The form's fields, in focus order.
    pub fn fields(self) -> &'static [Field] {
        use Field::*;
        match self {
            PlaylistKind::M3uUrl => &[PlaylistUrl, Name, EpgUrl],
            PlaylistKind::M3uFile => &[PlaylistFile, Name, EpgUrl],
            PlaylistKind::Xtream => &[Server, Username, Password, Name, EpgUrl],
            PlaylistKind::Stalker => &[PortalUrl, Mac, Name, EpgUrl],
        }
    }

    pub fn has_field(self, field: Field) -> bool {
        self.fields().contains(&field)
    }

    /// Where the content comes from: the field a network or parse failure
    /// points at.
    fn source_field(self) -> Field {
        match self {
            PlaylistKind::M3uUrl => Field::PlaylistUrl,
            PlaylistKind::M3uFile => Field::PlaylistFile,
            PlaylistKind::Xtream => Field::Server,
            PlaylistKind::Stalker => Field::PortalUrl,
        }
    }

    /// The account identity: the field an authentication failure points at.
    /// M3U credentials live inside the URL.
    fn auth_field(self) -> Field {
        match self {
            PlaylistKind::Xtream => Field::Username,
            PlaylistKind::Stalker => Field::Mac,
            other => other.source_field(),
        }
    }
}

/// One input of the parameters form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Field {
    PlaylistUrl,
    /// A path or content URI from the platform's file picker.
    PlaylistFile,
    Server,
    Username,
    Password,
    PortalUrl,
    Mac,
    /// Optional display name.
    Name,
    /// Optional; Xtream and Stalker portals usually provide their own guide.
    EpgUrl,
}

impl Field {
    const COUNT: usize = 9;

    /// Never shown in `Debug`; the shell masks it on screen.
    pub fn is_secret(self) -> bool {
        matches!(self, Field::Username | Field::Password | Field::Mac)
    }

    pub fn is_url(self) -> bool {
        matches!(
            self,
            Field::PlaylistUrl | Field::Server | Field::PortalUrl | Field::EpgUrl
        )
    }

    pub fn is_required(self) -> bool {
        !matches!(self, Field::Name | Field::EpgUrl)
    }

    /// Text fields take clipboard text; the file field is filled by the
    /// picker only.
    pub fn accepts_paste(self) -> bool {
        self != Field::PlaylistFile
    }
}

/// A credential. `Debug` prints `***`; read it with [`SecretText::expose`]
/// only where it goes on the wire or into secure storage.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct SecretText(String);

impl SecretText {
    pub fn new(value: impl Into<String>) -> Self {
        SecretText(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretText(***)")
    }
}

/// A URL that may embed credentials. `Debug` prints only its origin.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct UrlText(String);

impl UrlText {
    pub fn new(value: impl Into<String>) -> Self {
        UrlText(value.into())
    }

    /// The URL as entered (trimmed). Callers must not log it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for UrlText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UrlText({})", url_origin(&self.0))
    }
}

/// `scheme://host[:port]` with any userinfo, path, query and fragment gone.
fn url_origin(url: &str) -> String {
    let (scheme, rest) = match url.find("://") {
        Some(at) => (&url[..at + 3], &url[at + 3..]),
        None => ("", url),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    let more = if authority.len() < rest.len() || host.len() < authority.len() {
        "/…"
    } else {
        ""
    };
    format!("{scheme}{host}{more}")
}

/// Everything typed so far, for every field (kept when the type changes, so
/// going back and forth loses nothing). `Debug` redacts as described in the
/// module docs.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct FormValues([String; Field::COUNT]);

impl FormValues {
    pub fn get(&self, field: Field) -> &str {
        &self.0[field as usize]
    }

    pub fn set(&mut self, field: Field, value: impl Into<String>) {
        self.0[field as usize] = value.into();
    }
}

impl fmt::Debug for FormValues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut map = f.debug_map();
        for (i, value) in self.0.iter().enumerate() {
            if value.is_empty() {
                continue;
            }
            let field = FIELDS_BY_INDEX[i];
            if field.is_secret() {
                map.entry(&field, &format_args!("***"));
            } else if field.is_url() {
                map.entry(&field, &format_args!("{}", url_origin(value)));
            } else {
                map.entry(&field, value);
            }
        }
        map.finish()
    }
}

const FIELDS_BY_INDEX: [Field; Field::COUNT] = [
    Field::PlaylistUrl,
    Field::PlaylistFile,
    Field::Server,
    Field::Username,
    Field::Password,
    Field::PortalUrl,
    Field::Mac,
    Field::Name,
    Field::EpgUrl,
];

/// The provider-neutral result of the form; the app maps it to its provider
/// configuration. Values are trimmed (passwords excepted) and the MAC is
/// normalized to `AA:BB:CC:DD:EE:FF`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistDraft {
    pub source: DraftSource,
    pub name: Option<String>,
    pub epg_url: Option<UrlText>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftSource {
    M3uUrl {
        url: UrlText,
    },
    M3uFile {
        path: String,
    },
    /// `server` as typed; a missing scheme means `http`.
    Xtream {
        server: UrlText,
        username: SecretText,
        password: SecretText,
    },
    /// `portal` as typed; a missing scheme means `http`.
    Stalker {
        portal: UrlText,
        mac: SecretText,
    },
}

impl PlaylistDraft {
    pub fn kind(&self) -> PlaylistKind {
        match self.source {
            DraftSource::M3uUrl { .. } => PlaylistKind::M3uUrl,
            DraftSource::M3uFile { .. } => PlaylistKind::M3uFile,
            DraftSource::Xtream { .. } => PlaylistKind::Xtream,
            DraftSource::Stalker { .. } => PlaylistKind::Stalker,
        }
    }
}

/// Why a field was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldError {
    Required,
    ContainsWhitespace,
    /// The playlist and EPG URLs need `http://` or `https://`.
    MissingScheme,
    UnsupportedScheme,
    InvalidHost,
    InvalidPort,
    /// Not 12 hexadecimal digits.
    InvalidMac,
}

/// Checks the shape of an `http`/`https` URL: no whitespace, a known scheme
/// (optional when `scheme_required` is false), a host made of letter/digit/
/// hyphen labels or a bracketed IPv6 literal, and a port in 1..=65535.
/// Userinfo, path, query and fragment are not inspected.
pub fn validate_url(input: &str, scheme_required: bool) -> Result<(), FieldError> {
    if input.chars().any(char::is_whitespace) {
        return Err(FieldError::ContainsWhitespace);
    }
    let rest = match input.find("://") {
        Some(at) => {
            let scheme = input[..at].to_ascii_lowercase();
            if scheme != "http" && scheme != "https" {
                return Err(FieldError::UnsupportedScheme);
            }
            &input[at + 3..]
        }
        None if scheme_required => return Err(FieldError::MissingScheme),
        None => input,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");
    let (host_ok, port) = if let Some(v6) = host_port.strip_prefix('[') {
        let Some((addr, after)) = v6.split_once(']') else {
            return Err(FieldError::InvalidHost);
        };
        let ok = addr.contains(':')
            && addr
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.');
        let port = match after {
            "" => None,
            p => Some(p.strip_prefix(':').ok_or(FieldError::InvalidHost)?),
        };
        (ok, port)
    } else {
        let (host, port) = match host_port.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (host_port, None),
        };
        (valid_host_name(host), port)
    };
    if !host_ok {
        return Err(FieldError::InvalidHost);
    }
    if let Some(port) = port {
        let valid = !port.is_empty()
            && port.bytes().all(|b| b.is_ascii_digit())
            && port.parse::<u32>().is_ok_and(|p| (1..=65535).contains(&p));
        if !valid {
            return Err(FieldError::InvalidPort);
        }
    }
    Ok(())
}

fn valid_host_name(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.chars().count() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_alphanumeric() || c == '-')
        })
}

/// `00:1a:79:12:34:56`, `00-1A-79-12-34-56`, `001A.7912.3456` or
/// `001A79123456` → `00:1A:79:12:34:56`; `None` unless 12 hex digits.
pub fn normalize_mac(input: &str) -> Option<String> {
    let digits: String = input
        .chars()
        .filter(|c| !matches!(c, ':' | '-' | '.') && !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    if digits.len() != 12 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let pairs: Vec<&str> = (0..6).map(|i| &digits[i * 2..i * 2 + 2]).collect();
    Some(pairs.join(":"))
}

/// Validates one field's raw value (surrounding whitespace is ignored,
/// except that a password must not be empty).
pub fn validate_field(field: Field, value: &str) -> Result<(), FieldError> {
    let value = if field == Field::Password {
        value
    } else {
        value.trim()
    };
    if value.is_empty() {
        return if field.is_required() {
            Err(FieldError::Required)
        } else {
            Ok(())
        };
    }
    match field {
        Field::PlaylistUrl | Field::EpgUrl => validate_url(value, true),
        Field::Server | Field::PortalUrl => validate_url(value, false),
        Field::Mac => normalize_mac(value)
            .map(|_| ())
            .ok_or(FieldError::InvalidMac),
        _ => Ok(()),
    }
}

/// Correlates an import with its progress and result; results for any id
/// but the running one are ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImportId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportStage {
    Connecting,
    Downloading,
    Parsing,
    Saving,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportProgress {
    pub stage: ImportStage,
    /// Bytes or items, as the importer counts them.
    pub done: u64,
    pub total: Option<u64>,
}

impl ImportProgress {
    /// 0..=100 when the total is known.
    pub fn percent(&self) -> Option<u8> {
        let total = self.total.filter(|t| *t > 0)?;
        Some((u128::from(self.done.min(total)) * 100 / u128::from(total)) as u8)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportCounts {
    pub channels: u32,
    pub groups: u32,
    pub movies: u32,
    pub series: u32,
}

impl ImportCounts {
    /// Nothing playable arrived. Groups alone do not count.
    pub fn is_empty(&self) -> bool {
        self.channels == 0 && self.movies == 0 && self.series == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    /// The bounded import queue is full; retry after another import finishes.
    Busy,
    /// No connection, DNS, timeout, TLS, HTTP 5xx.
    Network,
    /// Local database, staging, or credential storage failed.
    Storage,
    /// The provider refused the account.
    Auth,
    /// The response or file is not a playlist.
    Parse,
    /// A valid playlist with nothing playable in it.
    EmptyPlaylist,
    /// A source this app cannot use (another portal type, a format we lack).
    Unsupported,
    /// The import was stopped before it finished.
    Canceled,
    /// This device cannot import playlists (a browser TV, whose host has no
    /// workers for imports yet).
    Unavailable,
}

impl FailureKind {
    /// Recoverable failures return to the form; others end on the failure
    /// screen, whose only way on is choosing another type.
    pub fn is_recoverable(self) -> bool {
        !matches!(self, FailureKind::Unsupported | FailureKind::Unavailable)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportFailure {
    pub kind: FailureKind,
    /// The importer's own pointer at the field at fault (an EPG URL that
    /// failed, say). Used when it belongs to the form; otherwise the
    /// wizard picks by [`FailureKind`].
    pub field: Option<Field>,
}

impl ImportFailure {
    pub fn new(kind: FailureKind) -> Self {
        ImportFailure { kind, field: None }
    }
}

/// Where the wizard was opened from: decides where Back from the form
/// goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// No playlists yet: the empty state is the root.
    FirstRun,
    /// "Add playlist" from settings: Back from the form closes.
    Settings,
}

/// The screen being shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Empty,
    ChooseType {
        focus: Target,
    },
    Form,
    /// The form is locked; Back or [`WizardEvent::Cancel`] stops it.
    Processing {
        id: ImportId,
        progress: Option<ImportProgress>,
    },
    Succeeded {
        id: ImportId,
        counts: ImportCounts,
    },
    /// An unrecoverable failure.
    Failed {
        failure: ImportFailure,
    },
    Closed,
}

/// The focused (or tapped) control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    AddPlaylist,
    RestoreBackup,
    SourceType,
    Kind(PlaylistKind),
    Field(Field),
    PastePlaylistLink,
    OptionalDetails,
    Submit,
    Back,
    CancelImport,
    Done,
    ChooseAnotherType,
}

#[derive(Clone, PartialEq, Eq)]
pub enum WizardEvent {
    Up,
    Down,
    Left,
    Right,
    Ok,
    Back,
    /// Touch: focus this control and activate it.
    Tap(Target),
    /// Text entry for `field` finished with `value`.
    SetField {
        field: Field,
        value: String,
    },
    /// Paste into the focused field.
    Paste,
    /// The clipboard read requested by [`WizardEffect::ReadClipboard`].
    ClipboardText {
        field: Field,
        text: Option<String>,
    },
    /// The picker's choice; `None` when dismissed.
    FilePicked(Option<String>),
    /// Stop the running import.
    Cancel,
    Progress {
        id: ImportId,
        progress: ImportProgress,
    },
    Succeeded {
        id: ImportId,
        counts: ImportCounts,
    },
    Failed {
        id: ImportId,
        failure: ImportFailure,
    },
}

impl std::fmt::Debug for WizardEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Up => f.write_str("Up"),
            Self::Down => f.write_str("Down"),
            Self::Left => f.write_str(stringify!(Left)),
            Self::Right => f.write_str(stringify!(Right)),
            Self::Ok => f.write_str("Ok"),
            Self::Back => f.write_str("Back"),
            Self::Tap(target) => f.debug_tuple("Tap").field(target).finish(),
            Self::SetField { field, .. } => f
                .debug_struct("SetField")
                .field("field", field)
                .field("value", &"[redacted]")
                .finish(),
            Self::Paste => f.write_str("Paste"),
            Self::ClipboardText { field, .. } => f
                .debug_struct("ClipboardText")
                .field("field", field)
                .field("text", &"[redacted]")
                .finish(),
            Self::FilePicked(_) => f.debug_tuple("FilePicked").field(&"[redacted]").finish(),
            Self::Cancel => f.write_str("Cancel"),
            Self::Progress { id, progress } => f
                .debug_struct("Progress")
                .field("id", id)
                .field("progress", progress)
                .finish(),
            Self::Succeeded { id, counts } => f
                .debug_struct("Succeeded")
                .field("id", id)
                .field("counts", counts)
                .finish(),
            Self::Failed { id, failure } => f
                .debug_struct("Failed")
                .field("id", id)
                .field("failure", failure)
                .finish(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WizardEffect {
    /// Open a backup picker while leaving welcome mounted underneath.
    RestoreBackup,
    /// Open text entry for `field`, prefilled with [`Wizard::value`];
    /// answer with [`WizardEvent::SetField`].
    EditField {
        field: Field,
    },
    /// Answer with [`WizardEvent::ClipboardText`].
    ReadClipboard {
        field: Field,
    },
    /// Answer with [`WizardEvent::FilePicked`].
    PickFile,
    /// Answer with progress, then one result, all carrying `id`.
    StartImport {
        id: ImportId,
        draft: PlaylistDraft,
    },
    CancelImport {
        id: ImportId,
    },
    Close(WizardExit),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WizardExit {
    /// The playlist from import `id` is ready.
    Added {
        id: ImportId,
    },
    Dismissed,
}

/// Short-lived feedback; cleared by the next event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    ClipboardEmpty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wizard {
    origin: Origin,
    step: Step,
    kind: PlaylistKind,
    values: FormValues,
    restore_available: bool,
    welcome_focus: Target,
    optional_expanded: bool,
    /// The form's focus, kept while the type list or processing is shown.
    form_focus: Target,
    errors: BTreeMap<Field, FieldError>,
    /// The last import's failure, shown on the form until the next submit.
    failure: Option<ImportFailure>,
    notice: Option<Notice>,
    next_id: u64,
}

impl Wizard {
    /// No playlists yet: start on the empty state.
    pub fn first_run() -> Self {
        Self::new(Origin::FirstRun, Step::Empty)
    }

    /// Enables the welcome screen's restore action for a capable host.
    pub fn with_restore(mut self, available: bool) -> Self {
        self.restore_available = available;
        self
    }

    /// From settings: start on the M3U link form.
    pub fn add_playlist() -> Self {
        Self::new(Origin::Settings, Step::Form)
    }

    fn new(origin: Origin, step: Step) -> Self {
        Wizard {
            origin,
            step,
            kind: PlaylistKind::M3uUrl,
            values: FormValues::default(),
            restore_available: false,
            welcome_focus: Target::AddPlaylist,
            optional_expanded: false,
            form_focus: Target::SourceType,
            errors: BTreeMap::new(),
            failure: None,
            notice: None,
            next_id: 1,
        }
    }

    pub fn origin(&self) -> Origin {
        self.origin
    }

    pub fn step(&self) -> Step {
        self.step
    }

    /// The chosen (or last chosen) type.
    pub fn kind(&self) -> PlaylistKind {
        self.kind
    }

    pub fn can_restore(&self) -> bool {
        self.restore_available
    }

    pub fn optional_expanded(&self) -> bool {
        self.optional_expanded
    }

    pub fn value(&self, field: Field) -> &str {
        self.values.get(field)
    }

    pub fn values(&self) -> &FormValues {
        &self.values
    }

    pub fn error(&self, field: Field) -> Option<FieldError> {
        self.errors.get(&field).copied()
    }

    pub fn failure(&self) -> Option<ImportFailure> {
        self.failure
    }

    pub fn notice(&self) -> Option<Notice> {
        self.notice
    }

    /// The focused control; `None` while processing (OK must not cancel by
    /// accident) and once closed.
    pub fn focus(&self) -> Option<Target> {
        match self.step {
            Step::Empty => Some(self.welcome_focus),
            Step::ChooseType { focus } => Some(focus),
            Step::Form => Some(self.form_focus),
            Step::Succeeded { .. } => Some(Target::Done),
            Step::Failed { .. } => Some(Target::ChooseAnotherType),
            Step::Processing { .. } | Step::Closed => None,
        }
    }

    pub fn handle(&mut self, event: WizardEvent) -> Option<WizardEffect> {
        self.notice = None;
        match self.step {
            Step::Empty => self.on_empty(event),
            Step::ChooseType { focus } => self.on_choose_type(focus, event),
            Step::Form => self.on_form(event),
            Step::Processing { id, .. } => self.on_processing(id, event),
            Step::Succeeded { id, .. } => match event {
                WizardEvent::Ok | WizardEvent::Back | WizardEvent::Tap(Target::Done) => {
                    self.step = Step::Closed;
                    Some(WizardEffect::Close(WizardExit::Added { id }))
                }
                _ => None,
            },
            Step::Failed { .. } => match event {
                WizardEvent::Ok
                | WizardEvent::Back
                | WizardEvent::Tap(Target::ChooseAnotherType) => {
                    self.open_source_picker();
                    None
                }
                _ => None,
            },
            Step::Closed => None,
        }
    }

    fn on_empty(&mut self, event: WizardEvent) -> Option<WizardEffect> {
        match event {
            WizardEvent::Back => self.close(),
            WizardEvent::Up => {
                self.welcome_focus = Target::AddPlaylist;
                None
            }
            WizardEvent::Down if self.restore_available => {
                self.welcome_focus = Target::RestoreBackup;
                None
            }
            WizardEvent::Ok => self.activate_welcome(self.welcome_focus),
            WizardEvent::Tap(target) => self.activate_welcome(target),
            _ => None,
        }
    }

    fn activate_welcome(&mut self, target: Target) -> Option<WizardEffect> {
        match target {
            Target::AddPlaylist => {
                self.form_focus = Target::SourceType;
                self.step = Step::Form;
                None
            }
            Target::RestoreBackup if self.restore_available => {
                self.welcome_focus = target;
                Some(WizardEffect::RestoreBackup)
            }
            _ => None,
        }
    }

    fn open_source_picker(&mut self) {
        self.form_focus = Target::SourceType;
        self.step = Step::ChooseType {
            focus: Target::Kind(self.kind),
        };
    }

    fn on_choose_type(&mut self, focus: Target, event: WizardEvent) -> Option<WizardEffect> {
        let target = match event {
            WizardEvent::Up | WizardEvent::Down => {
                let mut targets = PlaylistKind::ALL
                    .map(Target::Kind)
                    .into_iter()
                    .chain([Target::Back]);
                let at = targets.clone().position(|target| target == focus)?;
                let next = if event == WizardEvent::Up {
                    at.saturating_sub(1)
                } else {
                    at + 1
                };
                if let Some(focus) = targets.nth(next) {
                    self.step = Step::ChooseType { focus };
                }
                return None;
            }
            WizardEvent::Ok => focus,
            WizardEvent::Tap(target) => target,
            WizardEvent::Back => Target::Back,
            _ => return None,
        };
        match target {
            Target::Kind(kind) => self.choose(kind),
            Target::Back => self.step = Step::Form,
            _ => {}
        }
        None
    }

    /// A source selection moves to its first field. Switching sources
    /// keeps entered values and clears errors from the previous source.
    fn choose(&mut self, kind: PlaylistKind) {
        if kind != self.kind {
            self.kind = kind;
            self.errors.clear();
            self.failure = None;
        }
        self.form_focus = Target::Field(kind.source_field());
        self.step = Step::Form;
    }

    fn on_form(&mut self, event: WizardEvent) -> Option<WizardEffect> {
        match event {
            WizardEvent::Up => self.move_form_focus(-1),
            WizardEvent::Down => self.move_form_focus(1),
            WizardEvent::Left => match self.form_focus {
                Target::PastePlaylistLink => self.form_focus = Target::Field(Field::PlaylistUrl),
                Target::Back => self.form_focus = Target::Submit,
                _ => {}
            },
            WizardEvent::Right => match self.form_focus {
                Target::Field(Field::PlaylistUrl) => self.form_focus = Target::PastePlaylistLink,
                Target::Submit => self.form_focus = Target::Back,
                _ => {}
            },
            WizardEvent::Ok => return self.activate(self.form_focus),
            WizardEvent::Tap(target) if self.form_targets().any(|visible| visible == target) => {
                self.form_focus = target;
                return self.activate(target);
            }
            WizardEvent::SetField { field, value } => self.set_field(field, value),
            WizardEvent::Paste => {
                if let Target::Field(field) = self.form_focus {
                    return self.request_paste(field);
                }
            }
            WizardEvent::ClipboardText { field, text } => {
                match text.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                    Some(text) => {
                        let text = text.to_owned();
                        self.set_field(field, text);
                    }
                    None => self.notice = Some(Notice::ClipboardEmpty),
                }
            }
            WizardEvent::FilePicked(Some(path)) => self.set_field(Field::PlaylistFile, path),
            WizardEvent::Back => return self.back_from_form(),
            _ => {}
        }
        None
    }

    fn form_targets(&self) -> impl Iterator<Item = Target> + use<> {
        let optional_expanded = self.optional_expanded;
        let fields = self.kind.fields().iter().copied();
        std::iter::once(Target::SourceType)
            .chain(
                fields
                    .clone()
                    .filter(|field| field.is_required())
                    .map(Target::Field),
            )
            .chain((self.kind == PlaylistKind::M3uUrl).then_some(Target::PastePlaylistLink))
            .chain(std::iter::once(Target::OptionalDetails))
            .chain(
                fields
                    .filter(move |field| optional_expanded && !field.is_required())
                    .map(Target::Field),
            )
            .chain([Target::Submit, Target::Back])
    }

    fn move_form_focus(&mut self, step: isize) {
        let Some(at) = self
            .form_targets()
            .position(|target| target == self.form_focus)
        else {
            return;
        };
        if let Some(target) = self.form_targets().nth(at.saturating_add_signed(step)) {
            self.form_focus = target;
        }
    }

    fn activate(&mut self, target: Target) -> Option<WizardEffect> {
        match target {
            Target::SourceType => {
                self.open_source_picker();
                None
            }
            Target::Field(Field::PlaylistFile) => Some(WizardEffect::PickFile),
            Target::Field(field) => Some(WizardEffect::EditField { field }),
            Target::PastePlaylistLink => self.request_paste(Field::PlaylistUrl),
            Target::OptionalDetails => {
                self.optional_expanded = !self.optional_expanded;
                None
            }
            Target::Submit => self.submit(),
            Target::Back => self.back_from_form(),
            _ => None,
        }
    }

    fn back_from_form(&mut self) -> Option<WizardEffect> {
        match self.origin {
            Origin::FirstRun => {
                self.welcome_focus = Target::AddPlaylist;
                self.step = Step::Empty;
                None
            }
            Origin::Settings => self.close(),
        }
    }

    fn focus_field(&mut self, field: Field) {
        self.optional_expanded |= !field.is_required();
        self.form_focus = Target::Field(field);
    }

    fn set_field(&mut self, field: Field, value: String) {
        if !self.kind.has_field(field) {
            return;
        }
        // Only a field already marked wrong is re-checked while typing.
        if let Some(error) = self.errors.get_mut(&field) {
            match validate_field(field, &value) {
                Ok(()) => {
                    self.errors.remove(&field);
                }
                Err(e) => *error = e,
            }
        }
        self.values.set(field, value);
        self.focus_field(field);
    }

    fn request_paste(&mut self, field: Field) -> Option<WizardEffect> {
        (self.kind.has_field(field) && field.accepts_paste()).then(|| {
            self.focus_field(field);
            WizardEffect::ReadClipboard { field }
        })
    }

    fn submit(&mut self) -> Option<WizardEffect> {
        self.errors = self
            .kind
            .fields()
            .iter()
            .filter_map(|f| {
                validate_field(*f, self.values.get(*f))
                    .err()
                    .map(|e| (*f, e))
            })
            .collect();
        if let Some(first) = self
            .kind
            .fields()
            .iter()
            .find(|f| self.errors.contains_key(f))
        {
            self.focus_field(*first);
            return None;
        }
        let id = ImportId(self.next_id);
        self.next_id += 1;
        self.failure = None;
        self.step = Step::Processing { id, progress: None };
        Some(WizardEffect::StartImport {
            id,
            draft: self.draft(),
        })
    }

    /// The draft for the current values. Only meaningful once they validate.
    fn draft(&self) -> PlaylistDraft {
        let text = |f: Field| self.values.get(f).trim().to_owned();
        let optional = |f: Field| Some(text(f)).filter(|t| !t.is_empty());
        let source = match self.kind {
            PlaylistKind::M3uUrl => DraftSource::M3uUrl {
                url: UrlText(text(Field::PlaylistUrl)),
            },
            PlaylistKind::M3uFile => DraftSource::M3uFile {
                path: text(Field::PlaylistFile),
            },
            PlaylistKind::Xtream => DraftSource::Xtream {
                server: UrlText(text(Field::Server)),
                username: SecretText(text(Field::Username)),
                password: SecretText(self.values.get(Field::Password).to_owned()),
            },
            PlaylistKind::Stalker => DraftSource::Stalker {
                portal: UrlText(text(Field::PortalUrl)),
                mac: SecretText(normalize_mac(self.values.get(Field::Mac)).unwrap_or_default()),
            },
        };
        PlaylistDraft {
            source,
            name: optional(Field::Name),
            epg_url: optional(Field::EpgUrl).map(UrlText),
        }
    }

    fn on_processing(&mut self, running: ImportId, event: WizardEvent) -> Option<WizardEffect> {
        match event {
            WizardEvent::Cancel | WizardEvent::Back | WizardEvent::Tap(Target::CancelImport) => {
                self.fail(ImportFailure::new(FailureKind::Canceled));
                Some(WizardEffect::CancelImport { id: running })
            }
            WizardEvent::Progress { id, progress } if id == running => {
                self.step = Step::Processing {
                    id,
                    progress: Some(progress),
                };
                None
            }
            WizardEvent::Succeeded { id, counts } if id == running => {
                if counts.is_empty() {
                    self.fail(ImportFailure::new(FailureKind::EmptyPlaylist));
                } else {
                    self.step = Step::Succeeded { id, counts };
                }
                None
            }
            WizardEvent::Failed { id, failure } if id == running => {
                self.fail(failure);
                None
            }
            // OK, Submit, edits and stale results: the form is locked.
            _ => None,
        }
    }

    fn fail(&mut self, failure: ImportFailure) {
        self.failure = Some(failure);
        if !failure.kind.is_recoverable() {
            self.step = Step::Failed { failure };
            return;
        }
        match (failure.field, failure.kind) {
            (Some(field), _) if self.kind.has_field(field) => self.focus_field(field),
            (_, FailureKind::Canceled) => self.form_focus = Target::Submit,
            (_, FailureKind::Auth) => self.focus_field(self.kind.auth_field()),
            _ => self.focus_field(self.kind.source_field()),
        }
        self.step = Step::Form;
    }

    fn close(&mut self) -> Option<WizardEffect> {
        self.step = Step::Closed;
        Some(WizardEffect::Close(WizardExit::Dismissed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use FailureKind::*;
    use Field::*;
    use WizardEvent as E;

    fn set(w: &mut Wizard, field: Field, value: &str) {
        w.handle(E::SetField {
            field,
            value: value.into(),
        });
    }

    /// A wizard on `kind`'s form, every required field valid.
    fn filled(kind: PlaylistKind) -> Wizard {
        let mut w = Wizard::add_playlist();
        w.handle(E::Tap(Target::SourceType));
        w.handle(E::Tap(Target::Kind(kind)));
        match kind {
            PlaylistKind::M3uUrl => set(&mut w, PlaylistUrl, "http://iptv.example.test/list.m3u"),
            PlaylistKind::M3uFile => {
                w.handle(E::FilePicked(Some("/sdcard/Download/synthetic.m3u".into())));
            }
            PlaylistKind::Xtream => {
                set(&mut w, Server, "http://panel.example.test:8080");
                set(&mut w, Username, "synthetic-user");
                set(&mut w, Password, "s3cret-pass");
            }
            PlaylistKind::Stalker => {
                set(&mut w, PortalUrl, "http://portal.example.test/c/");
                set(&mut w, Mac, "00:1a:79:00:00:01");
            }
        }
        w
    }

    fn submit(w: &mut Wizard) -> Option<(ImportId, PlaylistDraft)> {
        match w.handle(E::Tap(Target::Submit)) {
            Some(WizardEffect::StartImport { id, draft }) => Some((id, draft)),
            _ => None,
        }
    }

    // --- Navigation and focus restoration (AC01, AC13) ---

    #[test]
    fn first_run_opens_the_default_form_from_welcome() {
        let mut w = Wizard::first_run();
        assert_eq!(w.step(), Step::Empty);
        assert_eq!(w.focus(), Some(Target::AddPlaylist));
        assert_eq!(w.handle(E::Up), None);
        assert_eq!(w.step(), Step::Empty);
        assert_eq!(w.handle(E::Ok), None);
        assert_eq!(w.step(), Step::Form);
        assert_eq!(w.kind(), PlaylistKind::M3uUrl);
        assert_eq!(w.focus(), Some(Target::SourceType));
    }

    #[test]
    fn restore_is_offered_only_on_hosts_with_the_capability() {
        let mut w = Wizard::first_run();
        assert_eq!(w.handle(E::Tap(Target::RestoreBackup)), None);
        w.handle(E::Down);
        assert_eq!(w.focus(), Some(Target::AddPlaylist));

        let mut w = Wizard::first_run().with_restore(true);
        w.handle(E::Down);
        assert_eq!(w.focus(), Some(Target::RestoreBackup));
        assert_eq!(w.handle(E::Ok), Some(WizardEffect::RestoreBackup));
        assert_eq!(w.step(), Step::Empty);
        assert_eq!(w.focus(), Some(Target::RestoreBackup));
        w.handle(E::Up);
        assert_eq!(
            w.handle(E::Tap(Target::RestoreBackup)),
            Some(WizardEffect::RestoreBackup)
        );
        assert_eq!(w.focus(), Some(Target::RestoreBackup));
        assert_eq!(w.handle(E::Tap(Target::AddPlaylist)), None);
        assert_eq!(w.focus(), Some(Target::SourceType));
    }

    #[test]
    fn back_from_the_empty_state_dismisses() {
        let mut w = Wizard::first_run();
        assert_eq!(
            w.handle(E::Back),
            Some(WizardEffect::Close(WizardExit::Dismissed))
        );
        assert_eq!(w.step(), Step::Closed);
        assert_eq!(w.focus(), None);
    }

    #[test]
    fn back_from_picker_returns_to_source_type_then_to_welcome() {
        let mut w = Wizard::first_run();
        w.handle(E::Ok);
        w.handle(E::Ok);
        for _ in PlaylistKind::ALL {
            w.handle(E::Down);
        }
        assert_eq!(w.focus(), Some(Target::Back));
        assert_eq!(w.handle(E::Ok), None);
        assert_eq!(w.step(), Step::Form);
        assert_eq!(w.focus(), Some(Target::SourceType));
        assert_eq!(w.handle(E::Back), None);
        assert_eq!(w.step(), Step::Empty);
        assert_eq!(w.focus(), Some(Target::AddPlaylist));
    }

    #[test]
    fn settings_entry_starts_on_the_default_form_and_back_dismisses() {
        let mut w = Wizard::add_playlist();
        assert_eq!(w.origin(), Origin::Settings);
        assert_eq!(w.focus(), Some(Target::SourceType));
        assert_eq!(
            w.handle(E::Back),
            Some(WizardEffect::Close(WizardExit::Dismissed))
        );
    }

    #[test]
    fn type_list_focus_moves_and_clamps() {
        let mut w = Wizard::add_playlist();
        w.handle(E::Ok);
        w.handle(E::Up);
        assert_eq!(w.focus(), Some(Target::Kind(PlaylistKind::M3uUrl)));
        for _ in 0..6 {
            w.handle(E::Down);
        }
        assert_eq!(w.focus(), Some(Target::Back));
        w.handle(E::Up);
        assert_eq!(w.focus(), Some(Target::Kind(PlaylistKind::Stalker)));
        w.handle(E::Up);
        assert_eq!(w.focus(), Some(Target::Kind(PlaylistKind::Xtream)));
        w.handle(E::Ok);
        assert_eq!(w.step(), Step::Form);
        assert_eq!(w.kind(), PlaylistKind::Xtream);
        assert_eq!(w.focus(), Some(Target::Field(Server)));
    }

    #[test]
    fn each_type_opens_its_own_fields_on_the_first_one() {
        let expected: [(PlaylistKind, &[Field]); 4] = [
            (PlaylistKind::M3uUrl, &[PlaylistUrl, Name, EpgUrl]),
            (PlaylistKind::M3uFile, &[PlaylistFile, Name, EpgUrl]),
            (
                PlaylistKind::Xtream,
                &[Server, Username, Password, Name, EpgUrl],
            ),
            (PlaylistKind::Stalker, &[PortalUrl, Mac, Name, EpgUrl]),
        ];
        for (kind, fields) in expected {
            assert_eq!(kind.fields(), fields);
            let mut w = Wizard::add_playlist();
            w.handle(E::Tap(Target::SourceType));
            w.handle(E::Tap(Target::Kind(kind)));
            assert_eq!(w.focus(), Some(Target::Field(fields[0])), "{kind:?}");
        }
    }

    #[test]
    fn form_focus_includes_only_disclosed_fields() {
        let mut w = Wizard::add_playlist();
        w.handle(E::Tap(Target::SourceType));
        w.handle(E::Tap(Target::Kind(PlaylistKind::Stalker)));
        w.handle(E::Up);
        assert_eq!(w.focus(), Some(Target::SourceType));
        for target in [
            Target::Field(PortalUrl),
            Target::Field(Mac),
            Target::OptionalDetails,
        ] {
            w.handle(E::Down);
            assert_eq!(w.focus(), Some(target));
        }
        assert!(!w.optional_expanded());
        assert_eq!(w.handle(E::Tap(Target::Field(Name))), None);
        w.handle(E::Down);
        assert_eq!(w.focus(), Some(Target::Submit));

        w.handle(E::Up);
        w.handle(E::Ok);
        assert!(w.optional_expanded());
        for target in [
            Target::Field(Name),
            Target::Field(EpgUrl),
            Target::Submit,
            Target::Back,
        ] {
            w.handle(E::Down);
            assert_eq!(w.focus(), Some(target));
        }
        w.handle(E::Down);
        assert_eq!(w.focus(), Some(Target::Back));
        w.handle(E::Left);
        assert_eq!(w.focus(), Some(Target::Submit));
    }

    #[test]
    fn choosing_the_same_source_keeps_entered_values() {
        let mut w = filled(PlaylistKind::Xtream);
        w.handle(E::Tap(Target::SourceType));
        assert_eq!(w.focus(), Some(Target::Kind(PlaylistKind::Xtream)));
        w.handle(E::Ok);
        assert_eq!(w.focus(), Some(Target::Field(Server)));
        assert_eq!(w.value(Username), "synthetic-user");
    }

    #[test]
    fn another_type_starts_fresh_focus_and_errors_but_keeps_shared_values() {
        let mut w = Wizard::add_playlist();
        w.handle(E::Tap(Target::SourceType));
        w.handle(E::Tap(Target::Kind(PlaylistKind::Xtream)));
        set(&mut w, Name, "Synthetic TV");
        submit(&mut w);
        assert!(w.error(Server).is_some());
        w.handle(E::Tap(Target::SourceType));
        w.handle(E::Up);
        w.handle(E::Ok);
        assert_eq!(w.kind(), PlaylistKind::M3uFile);
        assert_eq!(w.focus(), Some(Target::Field(PlaylistFile)));
        assert_eq!(w.error(Server), None);
        assert_eq!(w.value(Name), "Synthetic TV");
    }

    // --- Field entry, picker and clipboard ---

    #[test]
    fn ok_on_a_text_field_requests_text_entry_and_set_field_fills_it() {
        let mut w = Wizard::add_playlist();
        w.handle(E::Down);
        assert_eq!(
            w.handle(E::Ok),
            Some(WizardEffect::EditField { field: PlaylistUrl })
        );
        set(&mut w, PlaylistUrl, "http://a.example.test/x.m3u");
        assert_eq!(w.value(PlaylistUrl), "http://a.example.test/x.m3u");
    }

    #[test]
    fn set_field_moves_focus_there_and_ignores_foreign_fields() {
        let mut w = Wizard::add_playlist();
        set(&mut w, EpgUrl, "http://epg.example.test/guide.xml");
        assert!(w.optional_expanded());
        assert_eq!(w.focus(), Some(Target::Field(EpgUrl)));
        set(&mut w, Password, "nope");
        assert_eq!(w.value(Password), "");
        assert_eq!(w.focus(), Some(Target::Field(EpgUrl)));
        let mut fresh = Wizard::first_run();
        set(&mut fresh, PlaylistUrl, "http://a.example.test");
        assert_eq!(fresh.value(PlaylistUrl), "", "not on the form yet");
    }

    #[test]
    fn the_file_field_opens_the_picker_and_a_dismissed_picker_changes_nothing() {
        let mut w = Wizard::add_playlist();
        w.handle(E::Tap(Target::SourceType));
        w.handle(E::Tap(Target::Kind(PlaylistKind::M3uFile)));
        assert_eq!(w.handle(E::Ok), Some(WizardEffect::PickFile));
        w.handle(E::FilePicked(None));
        assert_eq!(w.value(PlaylistFile), "");
        w.handle(E::FilePicked(Some("content://docs/synthetic.m3u8".into())));
        assert_eq!(w.value(PlaylistFile), "content://docs/synthetic.m3u8");
        assert_eq!(w.focus(), Some(Target::Field(PlaylistFile)));
        assert_eq!(w.handle(E::Paste), None, "no paste into the file field");
    }

    #[test]
    fn a_picked_file_outside_the_file_form_is_ignored() {
        let mut w = Wizard::add_playlist();
        w.handle(E::FilePicked(Some("/x.m3u".into())));
        assert_eq!(w.value(PlaylistFile), "");
    }

    #[test]
    fn paste_reads_the_clipboard_for_the_focused_field_and_trims() {
        let mut w = Wizard::add_playlist();
        w.handle(E::Down);
        assert_eq!(
            w.handle(E::Paste),
            Some(WizardEffect::ReadClipboard { field: PlaylistUrl })
        );
        w.handle(E::ClipboardText {
            field: PlaylistUrl,
            text: Some("  http://p.example.test/get.m3u\n".into()),
        });
        assert_eq!(w.value(PlaylistUrl), "http://p.example.test/get.m3u");
        assert_eq!(w.notice(), None);
    }

    #[test]
    fn explicit_paste_is_only_for_playlist_links() {
        let mut w = Wizard::add_playlist();
        w.handle(E::Down);
        w.handle(E::Right);
        assert_eq!(w.focus(), Some(Target::PastePlaylistLink));
        assert_eq!(
            w.handle(E::Ok),
            Some(WizardEffect::ReadClipboard { field: PlaylistUrl })
        );
        assert_eq!(w.focus(), Some(Target::Field(PlaylistUrl)));

        w.handle(E::Tap(Target::SourceType));
        w.handle(E::Tap(Target::Kind(PlaylistKind::Stalker)));
        assert_eq!(w.handle(E::Tap(Target::PastePlaylistLink)), None);
        w.handle(E::Down);
        assert_eq!(
            w.handle(E::Paste),
            Some(WizardEffect::ReadClipboard { field: Mac })
        );
    }

    #[test]
    fn an_empty_clipboard_leaves_the_value_and_shows_a_notice_once() {
        let mut w = Wizard::add_playlist();
        set(&mut w, PlaylistUrl, "http://kept.example.test");
        for text in [None, Some(" \n ".to_owned())] {
            w.handle(E::ClipboardText {
                field: PlaylistUrl,
                text,
            });
            assert_eq!(w.notice(), Some(Notice::ClipboardEmpty));
            assert_eq!(w.value(PlaylistUrl), "http://kept.example.test");
        }
        w.handle(E::Down);
        assert_eq!(w.notice(), None);
    }

    #[test]
    fn paste_on_submit_does_nothing() {
        let mut w = Wizard::add_playlist();
        for _ in 0..4 {
            w.handle(E::Down);
        }
        assert_eq!(w.focus(), Some(Target::Submit));
        assert_eq!(w.handle(E::Paste), None);
    }

    // --- Validation ---

    #[test]
    fn required_fields_per_type() {
        for kind in PlaylistKind::ALL {
            let mut w = Wizard::add_playlist();
            w.handle(E::Tap(Target::SourceType));
            w.handle(E::Tap(Target::Kind(kind)));
            assert_eq!(submit(&mut w), None);
            for field in kind.fields() {
                let expected = field.is_required().then_some(FieldError::Required);
                assert_eq!(w.error(*field), expected, "{kind:?} {field:?}");
            }
            assert_eq!(w.step(), Step::Form);
        }
    }

    #[test]
    fn url_rules() {
        let ok = [
            "http://example.test",
            "HTTPS://Example.TEST:8443/path?q=1#f",
            "http://user:pw@example.test/get.php?username=a&password=b",
            "http://192.0.2.10:8080",
            "http://[2001:db8::1]:8080/x",
            "http://localhost/list.m3u",
            "http://xn--bcher-kva.example/",
            "http://bücher.example/",
        ];
        for url in ok {
            assert_eq!(validate_url(url, true), Ok(()), "{url}");
        }
        let bad = [
            ("example.test/list.m3u", FieldError::MissingScheme),
            ("ftp://example.test", FieldError::UnsupportedScheme),
            ("rtsp://example.test", FieldError::UnsupportedScheme),
            ("://example.test", FieldError::UnsupportedScheme),
            ("http://exa mple.test", FieldError::ContainsWhitespace),
            ("http://", FieldError::InvalidHost),
            ("http:///path", FieldError::InvalidHost),
            ("http://a..b", FieldError::InvalidHost),
            ("http://-a.test", FieldError::InvalidHost),
            ("http://a_b.test", FieldError::InvalidHost),
            ("http://a.test:", FieldError::InvalidPort),
            ("http://a.test:0", FieldError::InvalidPort),
            ("http://a.test:65536", FieldError::InvalidPort),
            ("http://a.test:80a", FieldError::InvalidPort),
            ("http://a.test:1:2", FieldError::InvalidPort),
            ("http://[2001:db8::1", FieldError::InvalidHost),
            ("http://[zz]", FieldError::InvalidHost),
            ("http://[2001:db8::1]x", FieldError::InvalidHost),
        ];
        for (url, err) in bad {
            assert_eq!(validate_url(url, true), Err(err), "{url}");
        }
    }

    #[test]
    fn server_and_portal_may_omit_the_scheme_but_playlist_and_epg_urls_may_not() {
        assert_eq!(validate_field(Server, "panel.example.test:8080"), Ok(()));
        assert_eq!(validate_field(PortalUrl, "portal.example.test/c/"), Ok(()));
        assert_eq!(
            validate_field(PlaylistUrl, "list.example.test/a.m3u"),
            Err(FieldError::MissingScheme)
        );
        assert_eq!(
            validate_field(EpgUrl, "epg.example.test/a.xml"),
            Err(FieldError::MissingScheme)
        );
        assert_eq!(
            validate_field(Server, "ftp://panel.example.test"),
            Err(FieldError::UnsupportedScheme)
        );
    }

    #[test]
    fn mac_rules() {
        for mac in [
            "00:1a:79:12:34:56",
            "00-1A-79-12-34-56",
            "001A.7912.3456",
            "001a79123456",
            " 00:1A:79:12:34:56 ",
        ] {
            assert_eq!(
                normalize_mac(mac).as_deref(),
                Some("00:1A:79:12:34:56"),
                "{mac}"
            );
            assert_eq!(validate_field(Mac, mac), Ok(()));
        }
        for mac in [
            "00:1A:79:12:34",
            "00:1A:79:12:34:56:78",
            "00:1A:79:12:34:5G",
            "x",
        ] {
            assert_eq!(normalize_mac(mac), None, "{mac}");
            assert_eq!(validate_field(Mac, mac), Err(FieldError::InvalidMac));
        }
        assert_eq!(validate_field(Mac, "  "), Err(FieldError::Required));
    }

    #[test]
    fn optional_fields_may_be_blank_but_a_present_epg_url_is_checked() {
        assert_eq!(validate_field(Name, ""), Ok(()));
        assert_eq!(validate_field(EpgUrl, "   "), Ok(()));
        assert_eq!(
            validate_field(EpgUrl, "epg"),
            Err(FieldError::MissingScheme)
        );
        let mut w = filled(PlaylistKind::M3uUrl);
        set(&mut w, EpgUrl, "nonsense");
        w.handle(E::Tap(Target::OptionalDetails));
        assert!(!w.optional_expanded());
        assert_eq!(submit(&mut w), None);
        assert!(w.optional_expanded());
        assert_eq!(w.error(EpgUrl), Some(FieldError::MissingScheme));
        assert_eq!(w.focus(), Some(Target::Field(EpgUrl)));
    }

    #[test]
    fn passwords_are_not_trimmed_but_must_not_be_empty() {
        assert_eq!(validate_field(Password, ""), Err(FieldError::Required));
        assert_eq!(validate_field(Password, " "), Ok(()));
        assert_eq!(validate_field(Username, "  "), Err(FieldError::Required));
    }

    #[test]
    fn submit_with_errors_focuses_the_first_invalid_field() {
        let mut w = Wizard::add_playlist();
        w.handle(E::Tap(Target::SourceType));
        w.handle(E::Tap(Target::Kind(PlaylistKind::Xtream)));
        set(&mut w, Server, "http://panel.example.test");
        set(&mut w, EpgUrl, "bad");
        assert_eq!(submit(&mut w), None);
        assert_eq!(w.focus(), Some(Target::Field(Username)));
        assert_eq!(w.error(Server), None);
        assert_eq!(w.error(Username), Some(FieldError::Required));
        assert_eq!(w.error(Password), Some(FieldError::Required));
        assert_eq!(w.error(EpgUrl), Some(FieldError::MissingScheme));
    }

    #[test]
    fn editing_a_field_with_an_error_revalidates_it() {
        let mut w = Wizard::add_playlist();
        w.handle(E::Tap(Target::SourceType));
        w.handle(E::Tap(Target::Kind(PlaylistKind::Stalker)));
        submit(&mut w);
        set(&mut w, Mac, "00:1A");
        assert_eq!(w.error(Mac), Some(FieldError::InvalidMac));
        set(&mut w, Mac, "00:1A:79:00:00:02");
        assert_eq!(w.error(Mac), None);
        assert_eq!(w.error(PortalUrl), Some(FieldError::Required), "untouched");
    }

    // --- Submit, drafts and no double submit ---

    #[test]
    fn submit_builds_a_trimmed_provider_neutral_draft_for_each_type() {
        let mut w = filled(PlaylistKind::Stalker);
        set(&mut w, PortalUrl, "  portal.example.test/c/ ");
        set(&mut w, Name, "  Portal one ");
        let (_, draft) = submit(&mut w).unwrap();
        assert_eq!(draft.kind(), PlaylistKind::Stalker);
        assert_eq!(
            draft,
            PlaylistDraft {
                source: DraftSource::Stalker {
                    portal: UrlText::new("portal.example.test/c/"),
                    mac: SecretText::new("00:1A:79:00:00:01"),
                },
                name: Some("Portal one".into()),
                epg_url: None,
            }
        );

        let mut w = filled(PlaylistKind::Xtream);
        set(&mut w, Password, " pass with spaces ");
        set(&mut w, EpgUrl, " http://epg.example.test/x.xml.gz ");
        let (_, draft) = submit(&mut w).unwrap();
        let DraftSource::Xtream {
            server,
            username,
            password,
        } = &draft.source
        else {
            panic!("{draft:?}")
        };
        assert_eq!(server.expose(), "http://panel.example.test:8080");
        assert_eq!(username.expose(), "synthetic-user");
        assert_eq!(password.expose(), " pass with spaces ");
        assert_eq!(
            draft.epg_url.as_ref().map(UrlText::expose),
            Some("http://epg.example.test/x.xml.gz")
        );
        assert_eq!(draft.name, None);

        let (_, draft) = submit(&mut filled(PlaylistKind::M3uUrl)).unwrap();
        assert_eq!(draft.kind(), PlaylistKind::M3uUrl);
        let (_, draft) = submit(&mut filled(PlaylistKind::M3uFile)).unwrap();
        assert_eq!(
            draft.source,
            DraftSource::M3uFile {
                path: "/sdcard/Download/synthetic.m3u".into()
            }
        );
    }

    #[test]
    fn processing_locks_the_form_and_ignores_every_second_submit() {
        let mut w = filled(PlaylistKind::M3uUrl);
        let (id, _) = submit(&mut w).unwrap();
        assert_eq!(w.step(), Step::Processing { id, progress: None });
        assert_eq!(w.focus(), None);
        for event in [
            E::Ok,
            E::Tap(Target::Submit),
            E::Tap(Target::Field(PlaylistUrl)),
            E::Paste,
            E::Up,
            E::Down,
            E::SetField {
                field: PlaylistUrl,
                value: "http://other.example.test".into(),
            },
        ] {
            assert_eq!(w.handle(event.clone()), None, "{event:?}");
            assert!(matches!(w.step(), Step::Processing { .. }));
        }
        assert_eq!(w.value(PlaylistUrl), "http://iptv.example.test/list.m3u");
    }

    #[test]
    fn progress_updates_only_for_the_running_import() {
        let mut w = filled(PlaylistKind::M3uUrl);
        let (id, _) = submit(&mut w).unwrap();
        let p = ImportProgress {
            stage: ImportStage::Downloading,
            done: 250,
            total: Some(1000),
        };
        w.handle(E::Progress {
            id: ImportId(id.0 + 7),
            progress: p,
        });
        assert_eq!(w.step(), Step::Processing { id, progress: None });
        w.handle(E::Progress { id, progress: p });
        assert_eq!(
            w.step(),
            Step::Processing {
                id,
                progress: Some(p)
            }
        );
        assert_eq!(p.percent(), Some(25));
    }

    #[test]
    fn progress_percent_needs_a_total_and_clamps() {
        let p = |done, total| ImportProgress {
            stage: ImportStage::Parsing,
            done,
            total,
        };
        assert_eq!(p(5, None).percent(), None);
        assert_eq!(p(5, Some(0)).percent(), None);
        assert_eq!(p(50, Some(10)).percent(), Some(100));
        assert_eq!(p(u64::MAX, Some(u64::MAX)).percent(), Some(100));
    }

    #[test]
    fn success_shows_counts_and_ok_closes_with_the_import() {
        let mut w = filled(PlaylistKind::Xtream);
        let (id, _) = submit(&mut w).unwrap();
        let counts = ImportCounts {
            channels: 120,
            groups: 9,
            movies: 40,
            series: 3,
        };
        w.handle(E::Succeeded {
            id: ImportId(99),
            counts,
        });
        assert!(matches!(w.step(), Step::Processing { .. }), "stale id");
        w.handle(E::Succeeded { id, counts });
        assert_eq!(w.step(), Step::Succeeded { id, counts });
        assert_eq!(w.focus(), Some(Target::Done));
        assert_eq!(w.handle(E::Up), None);
        assert_eq!(
            w.handle(E::Ok),
            Some(WizardEffect::Close(WizardExit::Added { id }))
        );
        assert_eq!(w.step(), Step::Closed);
    }

    #[test]
    fn back_on_success_also_closes_with_the_import() {
        let mut w = filled(PlaylistKind::M3uUrl);
        let (id, _) = submit(&mut w).unwrap();
        let counts = ImportCounts {
            movies: 1,
            ..Default::default()
        };
        w.handle(E::Succeeded { id, counts });
        assert_eq!(
            w.handle(E::Back),
            Some(WizardEffect::Close(WizardExit::Added { id }))
        );
    }

    #[test]
    fn a_closed_wizard_ignores_everything() {
        let mut w = Wizard::first_run();
        w.handle(E::Back);
        for event in [E::Ok, E::Back, E::Tap(Target::AddPlaylist), E::Cancel] {
            assert_eq!(w.handle(event), None);
            assert_eq!(w.step(), Step::Closed);
        }
    }

    // --- Failures retain input (AC02, AC11) ---

    #[test]
    fn a_playlist_with_nothing_playable_is_an_empty_playlist_failure() {
        let mut w = filled(PlaylistKind::M3uUrl);
        let (id, _) = submit(&mut w).unwrap();
        let counts = ImportCounts {
            groups: 4,
            ..Default::default()
        };
        w.handle(E::Succeeded { id, counts });
        assert_eq!(w.step(), Step::Form);
        assert_eq!(w.failure(), Some(ImportFailure::new(EmptyPlaylist)));
        assert_eq!(w.focus(), Some(Target::Field(PlaylistUrl)));
    }

    #[test]
    fn recoverable_failures_keep_type_and_values_and_focus_the_likely_field() {
        let cases = [
            (PlaylistKind::M3uUrl, Network, PlaylistUrl),
            (PlaylistKind::M3uUrl, Auth, PlaylistUrl),
            (PlaylistKind::M3uUrl, Parse, PlaylistUrl),
            (PlaylistKind::M3uFile, Parse, PlaylistFile),
            (PlaylistKind::M3uFile, EmptyPlaylist, PlaylistFile),
            (PlaylistKind::Xtream, Network, Server),
            (PlaylistKind::Xtream, Auth, Username),
            (PlaylistKind::Xtream, Parse, Server),
            (PlaylistKind::Xtream, EmptyPlaylist, Server),
            (PlaylistKind::Stalker, Network, PortalUrl),
            (PlaylistKind::Stalker, Auth, Mac),
            (PlaylistKind::Stalker, Parse, PortalUrl),
        ];
        for (kind, failure, focus) in cases {
            let mut w = filled(kind);
            let before = w.values().clone();
            let (id, _) = submit(&mut w).unwrap();
            w.handle(E::Failed {
                id,
                failure: ImportFailure::new(failure),
            });
            assert_eq!(w.step(), Step::Form, "{kind:?} {failure:?}");
            assert_eq!(w.kind(), kind);
            assert_eq!(w.values(), &before, "{kind:?} {failure:?}");
            assert_eq!(w.failure(), Some(ImportFailure::new(failure)));
            assert_eq!(
                w.focus(),
                Some(Target::Field(focus)),
                "{kind:?} {failure:?}"
            );
        }
    }

    #[test]
    fn the_password_survives_an_auth_failure_and_resubmits_unchanged() {
        let mut w = filled(PlaylistKind::Xtream);
        let (first, draft) = submit(&mut w).unwrap();
        w.handle(E::Failed {
            id: first,
            failure: ImportFailure::new(Auth),
        });
        assert_eq!(w.value(Password), "s3cret-pass");
        let (second, again) = submit(&mut w).unwrap();
        assert!(second > first);
        assert_eq!(again, draft);
        assert_eq!(w.failure(), None, "cleared by the new submit");
    }

    #[test]
    fn the_importers_field_hint_wins_when_it_is_on_the_form() {
        let mut w = filled(PlaylistKind::Xtream);
        let (id, _) = submit(&mut w).unwrap();
        w.handle(E::Failed {
            id,
            failure: ImportFailure {
                kind: Network,
                field: Some(EpgUrl),
            },
        });
        assert_eq!(w.focus(), Some(Target::Field(EpgUrl)));

        let (id, _) = submit(&mut w).unwrap();
        w.handle(E::Failed {
            id,
            failure: ImportFailure {
                kind: Auth,
                field: Some(Mac),
            },
        });
        assert_eq!(w.focus(), Some(Target::Field(Username)), "Mac is not on it");
    }

    #[test]
    fn a_stale_failure_does_not_touch_the_running_import() {
        let mut w = filled(PlaylistKind::M3uUrl);
        let (id, _) = submit(&mut w).unwrap();
        w.handle(E::Failed {
            id: ImportId(id.0 + 1),
            failure: ImportFailure::new(Network),
        });
        assert_eq!(w.step(), Step::Processing { id, progress: None });
        assert_eq!(w.failure(), None);
    }

    #[test]
    fn unsupported_ends_on_the_failure_screen_and_leads_back_to_the_type_list() {
        for exit in [E::Ok, E::Back, E::Tap(Target::ChooseAnotherType)] {
            let mut w = filled(PlaylistKind::Xtream);
            let (id, _) = submit(&mut w).unwrap();
            let failure = ImportFailure::new(Unsupported);
            w.handle(E::Failed { id, failure });
            assert_eq!(w.step(), Step::Failed { failure });
            assert_eq!(w.focus(), Some(Target::ChooseAnotherType));
            assert_eq!(w.handle(E::Down), None);
            w.handle(exit);
            assert_eq!(w.focus(), Some(Target::Kind(PlaylistKind::Xtream)));
            assert_eq!(w.value(Server), "http://panel.example.test:8080");
            w.handle(E::Ok);
            assert_eq!(w.value(Password), "s3cret-pass", "still there");
        }
    }

    // --- Cancel ---

    #[test]
    fn cancel_stops_the_import_and_returns_to_submit_with_values_kept() {
        for cancel in [E::Cancel, E::Back, E::Tap(Target::CancelImport)] {
            let mut w = filled(PlaylistKind::Stalker);
            let (id, _) = submit(&mut w).unwrap();
            assert_eq!(
                w.handle(cancel.clone()),
                Some(WizardEffect::CancelImport { id })
            );
            assert_eq!(w.step(), Step::Form);
            assert_eq!(w.focus(), Some(Target::Submit));
            assert_eq!(w.failure(), Some(ImportFailure::new(Canceled)));
            assert_eq!(w.value(Mac), "00:1a:79:00:00:01");
            // The importer's late answer for the canceled id is ignored.
            w.handle(E::Succeeded {
                id,
                counts: ImportCounts {
                    channels: 3,
                    ..Default::default()
                },
            });
            assert_eq!(w.step(), Step::Form, "{cancel:?}");
        }
    }

    #[test]
    fn ok_during_processing_never_cancels() {
        let mut w = filled(PlaylistKind::M3uUrl);
        submit(&mut w).unwrap();
        w.handle(E::Ok);
        w.handle(E::Ok);
        assert!(matches!(w.step(), Step::Processing { .. }));
    }

    #[test]
    fn an_importer_reported_cancel_returns_to_submit() {
        let mut w = filled(PlaylistKind::M3uUrl);
        let (id, _) = submit(&mut w).unwrap();
        assert_eq!(
            w.handle(E::Failed {
                id,
                failure: ImportFailure::new(Canceled)
            }),
            None
        );
        assert_eq!(w.focus(), Some(Target::Submit));
    }

    // --- Secrets ---

    #[test]
    fn debug_never_shows_credentials() {
        let mut w = filled(PlaylistKind::Xtream);
        set(
            &mut w,
            EpgUrl,
            "http://epguser:epgpass@epg.example.test/xmltv.php?password=epgpass",
        );
        let (_, draft) = submit(&mut w).unwrap();
        let mut m3u = filled(PlaylistKind::M3uUrl);
        set(
            &mut m3u,
            PlaylistUrl,
            "http://list.example.test/get.php?username=listuser&password=listpass",
        );
        let mut stalker = filled(PlaylistKind::Stalker);
        let (_, stalker_draft) = submit(&mut stalker).unwrap();
        let dumps = [
            format!("{w:?}"),
            format!("{draft:?}"),
            format!("{:?}", w.values()),
            format!("{m3u:?}"),
            format!("{stalker:?}"),
            format!("{stalker_draft:?}"),
        ];
        for dump in &dumps {
            for secret in [
                "s3cret-pass",
                "synthetic-user",
                "epguser",
                "epgpass",
                "listuser",
                "listpass",
                "00:1a:79:00:00:01",
                "00:1A:79:00:00:01",
            ] {
                assert!(!dump.contains(secret), "{secret} in {dump}");
            }
        }
        assert!(dumps[1].contains("panel.example.test:8080"));
        assert!(dumps[1].contains("UrlText(http://epg.example.test/…)"));
        assert!(dumps[3].contains("http://list.example.test/…"));
    }

    #[test]
    fn field_secrecy_and_url_classes() {
        let secret: Vec<Field> = FIELDS_BY_INDEX
            .into_iter()
            .filter(|f| f.is_secret())
            .collect();
        assert_eq!(secret, [Username, Password, Mac]);
        for (i, field) in FIELDS_BY_INDEX.iter().enumerate() {
            assert_eq!(*field as usize, i, "index table in sync");
        }
        assert_eq!(url_origin("example.test"), "example.test");
        assert_eq!(url_origin("http://u:p@h.test:81"), "http://h.test:81/…");
        assert_eq!(url_origin("https://h.test"), "https://h.test");
    }
}

#[cfg(test)]
mod event_diagnostic_regression {
    use super::*;
    #[test]
    fn entry_clipboard_and_file_events_redact_their_text() {
        for event in [
            WizardEvent::SetField {
                field: Field::Password,
                value: "synthetic-private-marker".into(),
            },
            WizardEvent::ClipboardText {
                field: Field::PlaylistUrl,
                text: Some("synthetic-private-marker".into()),
            },
            WizardEvent::FilePicked(Some("synthetic-private-marker".into())),
        ] {
            assert!(!format!("{event:?}").contains("synthetic-private-marker"));
        }
    }
}
