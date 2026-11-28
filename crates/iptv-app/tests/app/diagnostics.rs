//! End-to-end diagnostic log export (`general.send_logs`), confirmation dialog,
//! redaction of synthetic session secrets, and picker/share/clipboard delivery.

use super::personalization::step;
use crate::headless::Headless;
use iptv_settings::Settings;
use std::path::PathBuf;
use std::time::Duration;

fn app_data_root(home: &std::path::Path) -> PathBuf {
    home.join(if cfg!(windows) {
        "Patina"
    } else if cfg!(target_os = "macos") {
        "Library/Application Support/iptv-rs"
    } else {
        ".local/share/iptv-rs"
    })
}

fn open_settings(app: &mut Headless) {
    crate::personalization::open_program_menu(app);
    step(app.wait_for("text:Manage favorites…"));
    step(app.press("text:Settings >> nth=0"));
    step(app.wait_for("text:Clear history"));
}

#[test]
fn send_logs_confirms_and_exports_redacted_diagnostics_with_last_panic() {
    let Some(home) = isolated!() else { return };
    iptv_app::install_diagnostics();
    iptv_app::set_pickers_for_test(iptv_app::Pickers::BOTH);

    let root = app_data_root(home.path());
    let diag_dir = root.join("diagnostics");
    std::fs::create_dir_all(&diag_dir).unwrap();
    std::fs::write(
        diag_dir.join("last-panic.txt"),
        concat!(
            "app=v0.1.0(synthetichash)\n",
            "nsh_rev=00000000\n",
            "platform=linux\n",
            "target=x86_64-unknown-linux-gnu\n",
            "uptime=1.250s\n",
            "thread=main\n",
            "location=crates/iptv-app/src/main.rs:10:5\n",
            "message=synthetic prior crash at https://syn_old:syn_pw@old.example.test/stream.ts\n",
        ),
    )
    .unwrap();

    let mut fixture = crate::personalization::start(home, Settings::default(), false);
    let app = &mut fixture.app;

    log::info!(
        "iptv synthetic session event url=https://syn_live_user:syn_live_pass@provider.example.test:8080/live/42.ts path={}/iptv.sqlite",
        root.display()
    );
    log::warn!(
        "iptv synthetic auth failure password=syn_inline_secret user=syn_acct:syn_pw2@auth.example.test"
    );

    open_settings(app);
    step(app.wait_for("text:Data and privacy"));
    step(app.wait_for("text:Send a diagnostic log"));

    let exported_path = fixture.home.path().join("exported-diagnostics.txt");

    // Canceling the native save dialog reports Canceled without writing a file.
    step(app.on_pick_save_path(None));
    step(app.press("text:Send a diagnostic log"));
    step(app.wait_for("text:Send a diagnostic log?"));
    step(app.press_key("enter"));
    step(app.wait_for_within("text:Diagnostic log canceled", Duration::from_secs(10)));
    assert!(!exported_path.exists());

    step(app.on_pick_save_path(Some(exported_path.clone())));

    // Opening the command first presents the cataloged privacy confirmation;
    // Cancel in the confirmation dialog leaves the destination untouched.
    step(app.press("text:Send a diagnostic log"));
    step(app.wait_for("text:Send a diagnostic log?"));
    step(app.wait_for(
        "text:Includes the app version, platform, and redacted event log; no playlists, URLs or passwords.",
    ));
    step(app.press("text:Cancel"));
    step(app.wait_for_gone("text:Send a diagnostic log?", Duration::from_secs(3)));
    assert!(!exported_path.exists());

    // Confirming exports the header, prior panic and buffered redacted events.
    step(app.press("text:Send a diagnostic log"));
    step(app.wait_for("text:Send a diagnostic log?"));
    step(app.press_key("enter"));
    step(app.wait_for_within("text:Diagnostic log saved", Duration::from_secs(10)));

    let content = std::fs::read_to_string(&exported_path).unwrap();
    for expected in [
        "[patina-diagnostics]",
        "app=v",
        "schema=store:",
        "nsh_rev=",
        "platform=",
        "target=",
        "form_factor=",
        "window=",
        "uptime=",
        "[store-lanes]",
        "[last-panic]",
        "[events]",
        "previous run ended in a panic",
        "iptv synthetic session event",
        "[redacted-url]",
        "[redacted-path]",
    ] {
        assert!(
            content.contains(expected),
            "missing {expected:?} in exported diagnostics:\n{content}"
        );
    }
    for forbidden in [
        "provider.example.test",
        "old.example.test",
        "auth.example.test",
        "syn_live_user",
        "syn_live_pass",
        "syn_old",
        "syn_pw",
        "syn_inline_secret",
        "syn_pw2",
        "https://",
        &root.to_string_lossy(),
    ] {
        assert!(
            !content.contains(forbidden),
            "leaked {forbidden:?} in exported diagnostics:\n{content}"
        );
    }

    // Desktop runs also mirror redacted lines to `<data_dir>/diagnostics/patina.log`.
    if cfg!(any(target_os = "linux", target_os = "macos", windows)) {
        iptv_app::flush_diagnostics();
        let disk_log = std::fs::read_to_string(diag_dir.join("patina.log")).unwrap();
        assert!(
            disk_log.contains("iptv synthetic session event"),
            "{disk_log}"
        );
        assert!(!disk_log.contains("syn_live_pass"), "{disk_log}");
    }
}

#[test]
fn send_logs_without_create_picker_falls_back_to_share_or_clipboard() {
    let Some(home) = isolated!() else { return };
    iptv_app::install_diagnostics();
    iptv_app::set_pickers_for_test(iptv_app::Pickers::NONE);

    let mut fixture = crate::personalization::start(home, Settings::default(), false);
    let app = &mut fixture.app;

    log::info!(
        "iptv fallback export check https://syn_share:syn_secret@share.example.test/list.m3u"
    );

    open_settings(app);
    step(app.wait_for("text:Send a diagnostic log"));

    // Without a share sheet configured in headless mode, falls back to clipboard.
    step(app.press("text:Send a diagnostic log"));
    step(app.wait_for("text:Send a diagnostic log?"));
    step(app.press_key("enter"));
    step(app.wait_for_within(
        "text:Diagnostic log copied to the clipboard",
        Duration::from_secs(10),
    ));
    let clip = app.screen().clipboard().unwrap_or_default().to_owned();
    assert!(clip.contains("[patina-diagnostics]"), "{clip}");
    assert!(!clip.contains("syn_secret"), "{clip}");
    assert!(!clip.contains("share.example.test"), "{clip}");

    // Dismissing the system share sheet removes the staged temp file and notifies Canceled.
    step(app.on_share(nsh_core::share::ShareOutcome::dismissed()));
    step(app.press("text:Send a diagnostic log"));
    step(app.wait_for("text:Send a diagnostic log?"));
    step(app.press_key("enter"));
    step(app.wait_for_within("text:Diagnostic log canceled", Duration::from_secs(10)));
    let dismissed_file = PathBuf::from(&app.screen().shares()[0].files[0]);
    assert!(
        !dismissed_file.exists(),
        "dismissed share must clean up its staged file"
    );

    // Accepting two shares retains both unique staged attachments for asynchronous readers.
    step(app.on_share(nsh_core::share::ShareOutcome::shared(Some(
        "synthetic.mail".into(),
    ))));
    for _ in 0..2 {
        step(app.press("text:Send a diagnostic log"));
        step(app.wait_for("text:Send a diagnostic log?"));
        step(app.press_key("enter"));
        step(app.wait_for_within("text:Diagnostic log shared", Duration::from_secs(10)));
    }
    let shares = app.screen().shares();
    assert_eq!(shares.len(), 3);
    let first_shared = PathBuf::from(&shares[1].files[0]);
    let second_shared = PathBuf::from(&shares[2].files[0]);
    assert_ne!(first_shared, second_shared);
    assert!(
        first_shared.exists() && second_shared.exists(),
        "first shared attachment must remain readable after a second share"
    );
    let shared_text = std::fs::read_to_string(&first_shared).unwrap();
    assert!(
        shared_text.contains("[patina-diagnostics]"),
        "{shared_text}"
    );
    assert!(!shared_text.contains("syn_secret"), "{shared_text}");
}
