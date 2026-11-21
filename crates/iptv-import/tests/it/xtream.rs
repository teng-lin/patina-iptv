use crate::common;

use common::synth::{self, SimResponse, XT_PASS, XT_USER, XtreamSim};
use common::{FakeFetch, Reply, block_on, connect_error};
use iptv_import::{
    AuthProblem, CancelToken, Catalog, FailureKind, ImportError, ImportOptions, NoProgress,
    SourceMeta, Step,
};
use iptv_providers::Secret;
use iptv_providers::model::{ContentKind, SourceConfig, StreamLocator, XtreamCredentials};
use iptv_providers::xtream::AccountStatus;

fn config(pass: &str) -> SourceConfig {
    SourceConfig::Xtream(XtreamCredentials {
        server_url: "http://127.0.0.1:8090/".into(),
        username: Secret::new(XT_USER),
        password: Secret::new(pass),
        user_agent: None,
    })
}

fn fake(sim: XtreamSim) -> FakeFetch {
    FakeFetch::new(move |_, req| sim.handle(req).into())
}

fn run_with(fetch: &FakeFetch, opts: ImportOptions) -> Result<Catalog, ImportError> {
    block_on(iptv_import::import_source(
        fetch,
        &config(XT_PASS),
        &opts,
        &mut NoProgress,
        &CancelToken::new(),
    ))
}

fn run(fetch: &FakeFetch) -> Result<Catalog, ImportError> {
    run_with(fetch, ImportOptions::new(synth::NOW))
}

fn actions(fetch: &FakeFetch) -> Vec<String> {
    fetch
        .requests()
        .iter()
        .map(|r| {
            synth::SimRequest::parse(&r.url, Vec::new())
                .param("action")
                .unwrap_or("-")
                .to_owned()
        })
        .collect()
}

#[test]
fn live_import_without_vod_requests() {
    let fetch = fake(XtreamSim::default());
    let catalog = run(&fetch).unwrap();
    assert_eq!(catalog.channels.len(), 50);
    assert_eq!(catalog.groups.len(), 5);
    assert!(catalog.vod.is_empty() && catalog.series.is_empty());
    assert_eq!(
        actions(&fetch),
        [
            "get_account_info",
            "get_live_categories",
            "get_live_streams"
        ]
    );
    let ch = &catalog.channels[0];
    assert!(matches!(
        ch.stream,
        StreamLocator::Xtream { stream_id: 1, .. }
    ));
    assert_eq!(ch.groups[0], catalog.groups[1].id);
    let SourceMeta::Xtream {
        account,
        base_url,
        xmltv_url,
    } = &catalog.source
    else {
        panic!()
    };
    assert_eq!(base_url, "http://127.0.0.1:8090");
    let account = account.as_ref().unwrap();
    assert_eq!(account.status, AccountStatus::Active);
    assert_eq!(account.allowed_output_formats, ["m3u8", "ts"]);
    assert!(xmltv_url.expose().contains("xmltv.php"));
    assert_eq!(catalog.epg_sources(3).unwrap()[0].id.0, 3);
    assert_eq!(catalog.guide_urls(), std::slice::from_ref(xmltv_url));
    for req in fetch.requests() {
        let shown = format!("{req:?}");
        assert!(
            !shown.contains(XT_PASS) && !shown.contains(XT_USER),
            "{shown}"
        );
        assert!(
            req.header("user-agent").is_some(),
            "panels behind WAFs need a UA"
        );
    }
    assert!(!format!("{catalog:?}").contains(XT_PASS));
}

#[test]
fn vod_and_series_behind_flags() {
    let fetch = fake(XtreamSim::default());
    let catalog = run_with(&fetch, ImportOptions::new(synth::NOW).with_vod(true, true)).unwrap();
    assert_eq!(catalog.vod.len(), 20);
    assert_eq!(catalog.series.len(), 10);
    let kinds = |k| catalog.groups.iter().filter(|g| g.kind == k).count();
    assert_eq!(
        (
            kinds(ContentKind::Live),
            kinds(ContentKind::Movie),
            kinds(ContentKind::Series)
        ),
        (5, 5, 5)
    );
    assert_eq!(catalog.stats.requests, 7);
    assert!(catalog.warnings.is_empty(), "{:?}", catalog.warnings);
}

#[test]
fn optional_lists_fail_softly() {
    let fetch = fake(XtreamSim {
        fail_vod: Some(500),
        ..Default::default()
    });
    let catalog = run_with(&fetch, ImportOptions::new(synth::NOW).with_vod(true, false)).unwrap();
    assert_eq!(catalog.channels.len(), 50);
    assert!(catalog.vod.is_empty());
    assert!(
        catalog
            .warnings
            .iter()
            .any(|w| w.step == Step::Items(ContentKind::Movie) && w.message.contains("500"))
    );
    // Kept by kind: the empty movie list is a failure, not an empty catalog.
    assert!(matches!(
        catalog.on_demand_failures.as_slice(),
        [(ContentKind::Movie, ImportError::HttpStatus { .. })]
    ));
    let fine = fake(XtreamSim::default());
    let catalog = run_with(&fine, ImportOptions::new(synth::NOW).with_vod(true, true)).unwrap();
    assert!(catalog.on_demand_failures.is_empty());
}

#[test]
fn expired_and_inactive_accounts_are_auth_failures() {
    let fetch = fake(XtreamSim {
        exp_date: synth::NOW - 10,
        ..Default::default()
    });
    let err = run(&fetch).unwrap_err();
    assert!(
        matches!(
            &err,
            ImportError::Auth {
                problem: AuthProblem::XtreamAccount {
                    status: AccountStatus::Expired,
                    ..
                },
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(
        fetch.requests().len(),
        1,
        "no catalog requests after a refusal"
    );

    let err = block_on(iptv_import::import_source(
        &fake(XtreamSim::default()),
        &config("wrong-synthetic"),
        &ImportOptions::new(synth::NOW),
        &mut NoProgress,
        &CancelToken::new(),
    ))
    .unwrap_err();
    assert!(matches!(
        err,
        ImportError::Auth {
            problem: AuthProblem::XtreamAccount {
                status: AccountStatus::Inactive,
                ..
            },
            ..
        }
    ));
    assert_eq!(err.kind(), FailureKind::Auth);
}

#[test]
fn account_probes_fall_through_and_report_status() {
    // First probe 404, second answers.
    let sim = XtreamSim::default();
    let fetch = FakeFetch::new(move |_, req| match req.param("action") {
        Some("get_account_info") => SimResponse::status(404).into(),
        _ => sim.handle(req).into(),
    });
    let catalog = run(&fetch).unwrap();
    assert_eq!(catalog.channels.len(), 50);
    assert_eq!(&actions(&fetch)[..2], ["get_account_info", "-"]);

    let err = run(&fake(XtreamSim {
        fail_account: Some(401),
        ..Default::default()
    }))
    .unwrap_err();
    assert_eq!(
        err,
        ImportError::Auth {
            step: Step::AccountInfo,
            problem: AuthProblem::HttpStatus(401)
        }
    );
    let err = run(&fake(XtreamSim {
        fail_account: Some(502),
        ..Default::default()
    }))
    .unwrap_err();
    assert_eq!(
        err,
        ImportError::HttpStatus {
            step: Step::AccountInfo,
            status: 502
        }
    );
}

#[test]
fn panels_without_account_details_continue_with_a_warning() {
    let sim = XtreamSim::default();
    let fetch = FakeFetch::new(move |_, req| match req.param("action") {
        None | Some("get_account_info" | "get_profile") => SimResponse::ok("{}").into(),
        _ => sim.handle(req).into(),
    });
    let catalog = run(&fetch).unwrap();
    assert_eq!(catalog.channels.len(), 50);
    assert!(matches!(
        catalog.source,
        SourceMeta::Xtream { account: None, .. }
    ));
    assert_eq!(catalog.warnings[0].step, Step::AccountInfo);
}

#[test]
fn connection_failure_stops_probing() {
    let fetch = FakeFetch::new(|_, _| connect_error());
    let err = run(&fetch).unwrap_err();
    assert_eq!(err.kind(), FailureKind::Network);
    assert_eq!(fetch.requests().len(), 1);
}

#[test]
fn rejected_lists_and_garbage_are_not_empty() {
    let err = run(&fake(XtreamSim {
        reject_lists: true,
        ..Default::default()
    }))
    .unwrap_err();
    assert_eq!(
        err,
        ImportError::Auth {
            step: Step::Items(ContentKind::Live),
            problem: AuthProblem::XtreamRejected
        }
    );
    let sim = XtreamSim::default();
    let fetch = FakeFetch::new(move |_, req| match req.param("action") {
        Some("get_live_streams") => SimResponse::ok("<html>maintenance</html>").into(),
        _ => sim.handle(req).into(),
    });
    assert_eq!(run(&fetch).unwrap_err().kind(), FailureKind::Parse);
}

#[test]
fn an_empty_panel_is_empty() {
    let err = run(&fake(XtreamSim {
        channels: 0,
        ..Default::default()
    }))
    .unwrap_err();
    assert_eq!(
        err,
        ImportError::Empty {
            step: Step::Items(ContentKind::Live)
        }
    );
    // VOD-only accounts are not empty.
    let catalog = run_with(
        &fake(XtreamSim {
            channels: 0,
            ..Default::default()
        }),
        ImportOptions::new(synth::NOW).with_vod(true, false),
    )
    .unwrap();
    assert_eq!(catalog.vod.len(), 20);
}

#[test]
fn missing_categories_get_placeholder_groups() {
    let fetch = fake(XtreamSim {
        fail_categories: Some(500),
        ..Default::default()
    });
    let catalog = run(&fetch).unwrap();
    assert_eq!(catalog.channels.len(), 50);
    assert_eq!(catalog.groups.len(), 5);
    assert_eq!(catalog.stats.synthesized_groups, 5);
    assert!(
        catalog
            .groups
            .iter()
            .all(|g| g.name.starts_with("Category live"))
    );
    assert!(
        catalog
            .warnings
            .iter()
            .any(|w| w.step == Step::Categories(ContentKind::Live))
    );
}

#[test]
fn cancel_while_a_list_is_loading() {
    let cancel = CancelToken::new();
    let c = cancel.clone();
    let sim = XtreamSim::default();
    let fetch = FakeFetch::new(move |_, req| match req.param("action") {
        Some("get_live_streams") => Reply::Hang(Some(c.clone())),
        _ => sim.handle(req).into(),
    });
    let err = block_on(iptv_import::import_source(
        &fetch,
        &config(XT_PASS),
        &ImportOptions::new(synth::NOW),
        &mut NoProgress,
        &cancel,
    ));
    assert_eq!(err.unwrap_err(), ImportError::Canceled);
}

#[test]
fn optional_category_transport_failure_keeps_live_catalog() {
    let sim = XtreamSim::default();
    let fetch = FakeFetch::new(move |_, req| match req.param("action") {
        Some("get_vod_categories" | "get_series_categories") => connect_error(),
        _ => sim.handle(req).into(),
    });
    let catalog = run_with(&fetch, ImportOptions::new(synth::NOW).with_vod(true, true)).unwrap();
    assert_eq!(catalog.channels.len(), 50);
    assert!(!catalog.vod.is_empty());
    assert!(
        catalog
            .vod
            .iter()
            .flat_map(|v| &v.groups)
            .all(|id| catalog.groups.iter().any(|g| &g.id == id)),
        "missing categories are synthesized from imported rows"
    );
    assert!(
        catalog
            .warnings
            .iter()
            .any(|w| w.step == Step::Categories(ContentKind::Movie))
    );
}
