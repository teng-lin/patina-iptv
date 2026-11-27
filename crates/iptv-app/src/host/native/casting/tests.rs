use super::*;

#[test]
fn cast_relay_uses_a_fixed_default_or_an_explicit_tcp_port() {
    assert_eq!(configured_relay_port(None), Ok(49152));
    assert_eq!(configured_relay_port(Some(OsStr::new("53317"))), Ok(53317));
    for value in ["", "0", "65536", "abc"] {
        assert_eq!(
            configured_relay_port(Some(OsStr::new(value))),
            Err(Error::Unavailable)
        );
    }
}
