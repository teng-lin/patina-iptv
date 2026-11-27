//! A dialog and its asynchronous request epoch, including when it is closed.

#[derive(Clone, PartialEq, Eq)]
pub struct State<T> {
    pub current: Option<T>,
    request_epoch: u64,
}

impl<T> Default for State<T> {
    fn default() -> Self {
        Self {
            current: None,
            request_epoch: 0,
        }
    }
}

impl<T> State<T> {
    pub fn epoch(&self) -> u64 {
        self.request_epoch
    }

    pub fn next_request(&mut self) -> u64 {
        self.request_epoch = self.request_epoch.wrapping_add(1);
        self.request_epoch
    }

    pub fn open(&mut self, value: T) -> u64 {
        self.current = Some(value);
        self.next_request()
    }

    pub fn close(&mut self) -> bool {
        self.next_request();
        self.current.take().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_epochs_remain_distinct_after_closing_and_reopening() {
        let mut state = State::default();
        let first = state.open(1);
        let request = state.next_request();
        assert!(request > first);
        assert!(state.close());
        let next = state.open(1);
        assert!(next > request);
        assert_eq!(state.epoch(), next);
    }
}
