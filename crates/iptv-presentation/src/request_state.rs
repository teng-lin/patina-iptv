//! The request epoch of a reusable dialog holder, including while closed.

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Epoch {
    request_epoch: u64,
}

impl Epoch {
    pub fn current(&self) -> u64 {
        self.request_epoch
    }

    /// Advance on replacement, dismissal, or the start of a competing request.
    pub fn advance(&mut self) -> u64 {
        self.request_epoch = self.request_epoch.wrapping_add(1);
        self.request_epoch
    }
}
