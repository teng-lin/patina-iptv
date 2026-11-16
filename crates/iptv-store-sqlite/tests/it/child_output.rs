use crate::initialization_support;
use std::{io::Write, time::Duration};

#[test]
fn verbose_child_output_is_collected_without_blocking() {
    const CHILD: &str = "IPTV_VERBOSE_TEST_CHILD";
    const BYTES: usize = 256 << 10;
    if std::env::var_os(CHILD).is_some() {
        std::io::stdout().write_all(&vec![b'x'; BYTES]).unwrap();
        std::io::stderr().write_all(&vec![b'y'; BYTES]).unwrap();
        return;
    }
    let output = initialization_support::child(
        "child_output::verbose_child_output_is_collected_without_blocking",
        CHILD,
        Duration::from_secs(20),
    );
    assert!(output.status.success(), "verbose child failed");
    assert!(output.stdout.len() >= BYTES, "stdout was truncated");
    assert!(output.stderr.len() >= BYTES, "stderr was truncated");
}
