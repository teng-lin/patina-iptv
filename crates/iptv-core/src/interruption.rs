//! Reports from a player host when the system interrupts its audio.

/// What the host reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interruption {
    /// Another app took the audio focus for good.
    FocusLoss,
    /// Another app took it for a while (a call, a navigation prompt).
    FocusLossTransient,
    /// The audio focus came back.
    FocusGain,
    /// The audio's route went away (headphones, a Bluetooth headset).
    Noisy,
}
