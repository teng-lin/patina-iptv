//! Decode the credential result returned by Android's Keystore bridge.
use iptv_cast::{Credential, StoreError};

pub(super) fn decode_credential(mut payload: Vec<u8>) -> Result<Option<Credential>, StoreError> {
    match payload.as_slice() {
        [0] => Ok(None),
        [1, value @ ..] if !value.is_empty() && value.len() <= 4096 => {
            // Retain the allocation instead of making a second plaintext copy.
            payload.remove(0);
            Ok(Some(Credential::from_bytes(payload)))
        }
        _ => {
            payload.fill(0);
            Err(StoreError::invalid_credential())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_missing_marker_allows_pairing_again() {
        assert!(decode_credential(vec![0]).unwrap().is_none());
        for malformed in [vec![], vec![0, 4], vec![1], vec![2], vec![3, 4]] {
            assert!(decode_credential(malformed).is_err());
        }
    }

    #[test]
    fn accepts_binary_credential_up_to_the_android_limit() {
        let bytes = vec![0x96; 4096];
        let payload = [&[1], bytes.as_slice()].concat();
        let credential = decode_credential(payload.clone()).unwrap().unwrap();
        assert_eq!(credential.as_bytes(), bytes);
        let mut oversized = payload;
        oversized.push(0);
        assert!(decode_credential(oversized).is_err());
    }
}
