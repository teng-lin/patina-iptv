//! An interactive sender for physical checks with generated HLS or MP4.
use iptv_cast::{
    Cast, Connect, Connection, Diagnostic, Link, Media, PeerTrust, PinCode, Playback, Route,
};
use std::{error::Error, io, num::NonZeroU16, time::Duration};
use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn list_routes(routes: &[&Route]) {
    for (index, route) in routes.iter().enumerate() {
        println!("{} {}", index + 1, route.name());
    }
}

async fn connect(cast: &Cast, route: &Route) -> Result<Connection> {
    Ok(match cast.connect(route).await? {
        Connect::Ready(connection) => connection,
        Connect::PinRequired(pairing) => {
            let pin = tokio::task::spawn_blocking(|| {
                rpassword::prompt_password(
                    "Approve the request on the receiver, then enter its PIN: ",
                )
            })
            .await??;
            let pin = Zeroizing::new(pin);
            let parsed = PinCode::parse(pin.trim());
            drop(pin);
            match parsed {
                Ok(pin) => pairing.submit(pin).await?,
                Err(error) => {
                    pairing.cancel().await;
                    return Err(error.into());
                }
            }
        }
        _ => return Err("The selected route requires an unsupported pairing method".into()),
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let timing_port = std::env::var("PATINA_AIRPLAY_TIMING_PORT")
        .ok()
        .map(|value| value.parse::<NonZeroU16>())
        .transpose()?;
    let cast = Cast::builder().airplay_timing_port(timing_port).build()?;
    let discovery = cast.discover(Duration::from_secs(3)).await?;
    for issue in discovery.issues() {
        eprintln!(
            "{} discovery failed: {}",
            issue.protocol().as_str(),
            issue.error()
        );
    }
    let mut routes: Vec<_> = discovery
        .routes()
        .iter()
        .filter(|route| route.protocol().as_str() == "airplay")
        .collect();
    routes.sort_by(|left, right| {
        left.name()
            .cmp(right.name())
            .then(left.id().as_str().cmp(right.id().as_str()))
    });
    list_routes(&routes);
    if arguments == ["--list"] {
        return Ok(());
    }

    let [mode, url, seconds] = arguments.as_slice() else {
        return Err(
            "Usage: airplay_probe --list | (live|vod) SYNTHETIC_URL OBSERVATION_SECONDS".into(),
        );
    };
    let media = match mode.as_str() {
        "live" => Media::live(url),
        "vod" => Media::on_demand(url),
        _ => return Err("Choose live or vod".into()),
    }?;
    let seconds: NonZeroU16 = seconds.parse()?;
    println!("Enter the receiver number:");
    let mut selected = String::new();
    io::stdin().read_line(&mut selected)?;
    let selection: NonZeroU16 = selected.trim().parse()?;
    let Some(route) = routes.get(usize::from(selection.get() - 1)) else {
        return Err("No such receiver in the current scan".into());
    };

    let connection = connect(&cast, route).await?;
    if !matches!(connection.peer_trust(), PeerTrust::VerifiedPairing) {
        connection.close().await;
        return Err("Receiver identity could not be verified".into());
    }
    let session = connection.load(media)?;
    let mut updates = session.subscribe();
    let mut playing = false;
    let mut ended = false;
    let mut live_playback = false;
    let observed = tokio::time::timeout(Duration::from_secs(seconds.get().into()), async {
        while let Some(snapshot) = updates.next().await {
            println!(
                "{:?} {:?} position={:?} duration={:?}",
                snapshot.link(),
                snapshot.playback(),
                snapshot.position(),
                snapshot.duration()
            );
            let error = match snapshot.playback() {
                Playback::Failed(error) => Some(error),
                _ => snapshot.link_error(),
            };
            if let Some(error) = error {
                eprintln!("{error}");
                if let Some(Diagnostic::TimingListener { local_addr }) = error.diagnostic() {
                    eprintln!("Check whether the receiver can reach the sender's UDP timing listener at {local_addr}.");
                }
            }
            playing |= matches!(snapshot.playback(), Playback::Playing);
            live_playback = snapshot.link() == Link::Connected && matches!(snapshot.playback(), Playback::Playing);
            ended |= matches!(snapshot.playback(), Playback::Ended);
        }
    })
    .await;
    println!("Stop outcome: {:?}", session.stop().await?);
    // A second connection in the same sender must verify without any new PIN.
    match cast.connect(route).await? {
        Connect::Ready(connection) => connection.close().await,
        Connect::PinRequired(pairing) => {
            pairing.cancel().await;
            return Err("Receiver requested a second PIN for the saved pairing".into());
        }
        _ => return Err("Receiver offered an unexpected pairing method".into()),
    }
    if !playing {
        return Err("No receiver-confirmed playback during the observation window".into());
    }
    if mode == "vod" && (observed.is_err() || !ended) {
        return Err("VOD playback did not finish naturally within the observation window".into());
    }
    if mode == "live" && (observed.is_ok() || !live_playback) {
        return Err("Live playback did not stay active through the observation window".into());
    }
    Ok(())
}
