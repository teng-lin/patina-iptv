//! One request from the network worker to the window that chose a receiver.
use futures_channel::oneshot;
use iptv_core::cast::Error;
use zeroize::Zeroizing;

pub struct PairingPrompt(oneshot::Sender<PairingRequest>);

pub struct PairingRequest(oneshot::Sender<Zeroizing<String>>);

pub fn pairing_prompt() -> (PairingPrompt, oneshot::Receiver<PairingRequest>) {
    let (send, receive) = oneshot::channel();
    (PairingPrompt(send), receive)
}

impl PairingPrompt {
    /// Ask the owner for a receiver PIN. Dropping either owner cancels the request.
    pub async fn request(self) -> Result<Zeroizing<String>, Error> {
        let (send, receive) = oneshot::channel();
        self.0
            .send(PairingRequest(send))
            .map_err(|_| Error::Cancelled)?;
        receive.await.map_err(|_| Error::Cancelled)
    }
}

impl PairingRequest {
    /// Submission can race expiry, grant revocation or receiver disconnect.
    pub fn submit(self, pin: Zeroizing<String>) -> Result<(), Error> {
        self.0.send(pin).map_err(|_| Error::Cancelled)
    }
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn submission_preserves_leading_zeroes() {
        let (prompt, receive) = pairing_prompt();
        let waiting = tokio::spawn(prompt.request());
        receive
            .await
            .unwrap_or_else(|_| panic!("pairing request was lost"))
            .submit(Zeroizing::new("0123".into()))
            .unwrap();
        assert_eq!(waiting.await.unwrap().unwrap().as_str(), "0123");
    }

    #[tokio::test]
    async fn dismissing_or_removing_the_window_cancels_the_request() {
        let (prompt, receive) = pairing_prompt();
        drop(receive);
        assert_eq!(prompt.request().await.unwrap_err(), Error::Cancelled);

        let (prompt, receive) = pairing_prompt();
        let waiting = tokio::spawn(prompt.request());
        drop(
            receive
                .await
                .unwrap_or_else(|_| panic!("no pairing request")),
        );
        assert_eq!(waiting.await.unwrap().unwrap_err(), Error::Cancelled);
    }

    #[tokio::test]
    async fn dropping_the_worker_rejects_late_submission() {
        let (prompt, receive) = pairing_prompt();
        let waiting = tokio::spawn(prompt.request());
        let request = receive
            .await
            .unwrap_or_else(|_| panic!("no pairing request"));
        waiting.abort();
        assert!(waiting.await.unwrap_err().is_cancelled());
        assert_eq!(
            request.submit(Zeroizing::new("0123".into())),
            Err(Error::Cancelled)
        );
    }
}
