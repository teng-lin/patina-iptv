use super::*;

fn overlay(shm_dir: Option<TempDir>) -> Arc<KittyOverlay> {
    let mut overlay = KittyOverlay::new();
    Arc::get_mut(&mut overlay)
        .expect("the overlay has one owner before use")
        .shm_dir = shm_dir;
    overlay
}

#[test]
fn inline_video_draws_both_scanlines_and_clears_the_frame_in_guide_mode() {
    let mut player = TuiPlayer::new(VideoMode::Inline, overlay(None));
    let fullscreen = Rect::new(0, 0, 16, 8);
    let mut buffer = Buffer::empty(fullscreen);
    *player.frame.lock().expect("frame lock") = Some(Arc::new(RgbFrame {
        width: 1,
        height: 2,
        data: vec![1, 2, 3, 4, 5, 6],
        seq: 1,
    }));

    player.render(fullscreen, &mut buffer);
    for x in 0..fullscreen.width {
        assert_eq!(buffer[(x, 0)].symbol(), "▀");
        assert_eq!(buffer[(x, 0)].fg, Color::Rgb(1, 2, 3));
        assert_eq!(buffer[(x, 7)].bg, Color::Rgb(4, 5, 6));
    }

    player.render(Rect::new(0, 0, 4, 2), &mut buffer);
    assert!(player.frame.lock().expect("frame lock").is_none());
    player.render(fullscreen, &mut buffer);
    let row: String = (0..fullscreen.width)
        .map(|x| buffer[(x, 4)].symbol())
        .collect();
    assert_eq!(row.trim(), "[No stream]");
}

#[test]
fn terminal_player_updates_pause_without_replacing_the_same_source_token() {
    fn source(token: u64, header: &str) -> (Arc<str>, Option<JsonValue>) {
        let headers = JsonValue::Object(
            [
                ("User-Agent".into(), JsonValue::String(header.into())),
                ("Ignored".into(), JsonValue::Bool(true)),
            ]
            .into_iter()
            .collect(),
        );
        let props = JsonValue::Object(
            [
                (
                    "url".into(),
                    JsonValue::String("http://127.0.0.1:19999/demo.ts".into()),
                ),
                ("token".into(), JsonValue::from(token)),
                ("headers".into(), headers),
            ]
            .into_iter()
            .collect(),
        );
        ("source".into(), Some(props))
    }

    let area = Rect::new(0, 0, 16, 8);
    let mut buffer = Buffer::empty(area);
    let mut player = TuiPlayer::new(VideoMode::None, overlay(None));
    player.set_props(&[source(1, "Synthetic/1")]);
    player.set_props(&[
        source(1, "Synthetic/2"),
        ("paused".into(), Some(JsonValue::Bool(true))),
        ("muted".into(), Some(JsonValue::Bool(true))),
    ]);
    let current = player.current_source.as_ref().expect("original source");
    assert_eq!(
        current.headers,
        [("User-Agent".into(), "Synthetic/1".into())]
    );
    assert!(player.muted);
    let mut status = |player: &mut TuiPlayer| {
        player.render(area, &mut buffer);
        let row: String = (0..area.width).map(|x| buffer[(x, 4)].symbol()).collect();
        row.trim().to_owned()
    };
    assert_eq!(status(&mut player), "[Paused]");

    player.set_props(&[source(2, "Synthetic/2"), ("paused".into(), None)]);
    let current = player.current_source.as_ref().expect("replacement source");
    assert_eq!(current.token, Some(2));
    assert_eq!(
        current.headers,
        [("User-Agent".into(), "Synthetic/2".into())]
    );
    assert_eq!(status(&mut player), "[Video off]");

    player.set_props(&[("source".into(), None)]);
    assert_eq!(status(&mut player), "[No stream]");
}

#[test]
fn kitty_inline_upload_reassembles_the_frame_and_replaces_the_previous_image() {
    use flate2::read::ZlibDecoder;

    let overlay = overlay(None);
    let rect = Rect::new(3, 2, 12, 8);
    *overlay.rect.lock().expect("overlay bounds lock") = Some(rect);
    overlay.active.store(true, Ordering::Relaxed);
    let mut seed = 7_u32;
    let data = (0..48 * 48 * 3)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 24) as u8
        })
        .collect();
    let frame = Arc::new(RgbFrame {
        width: 48,
        height: 48,
        data,
        seq: 1,
    });
    *overlay.frame.lock().expect("overlay frame lock") = Some(frame.clone());

    let mut out = Vec::new();
    overlay.flush(&mut out).expect("first terminal upload");
    let output = std::str::from_utf8(&out).expect("Kitty command text");
    assert!(output.starts_with("\x1b[?2026h\x1b7\x1b[3;4H"));
    assert!(output.ends_with("\x1b8\x1b[?2026l"));
    let chunks: Vec<_> = output
        .split("\x1b_G")
        .skip(1)
        .map(|command| {
            command
                .split_once("\x1b\\")
                .expect("terminated graphics command")
                .0
        })
        .collect();
    assert!(chunks[0].starts_with("a=T,t=d,f=24,o=z,s=48,v=48,c=12,r=8,i=42,"));
    assert!(
        chunks[0].contains("m=1;"),
        "the frame must span multiple chunks"
    );
    assert!(chunks.last().expect("final chunk").starts_with("m=0,"));
    let payload: String = chunks
        .iter()
        .map(|chunk| chunk.split_once(';').expect("chunk payload").1)
        .collect();
    let compressed = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .expect("base64 payload");
    let mut pixels = Vec::new();
    ZlibDecoder::new(compressed.as_slice())
        .read_to_end(&mut pixels)
        .expect("RGB payload");
    assert_eq!(pixels, frame.data);

    out.clear();
    overlay.flush(&mut out).expect("unchanged frame");
    assert!(
        out.is_empty(),
        "an unchanged frame should use no terminal bandwidth"
    );

    *overlay.rect.lock().expect("overlay bounds lock") = Some(Rect { x: 4, ..rect });
    overlay.flush(&mut out).expect("repositioned frame");
    let output = std::str::from_utf8(&out).expect("Kitty command text");
    let next = output.find("i=43,").expect("replacement image upload");
    let delete = output
        .find("a=d,d=I,i=42,")
        .expect("previous image deletion");
    assert!(
        next < delete,
        "the old image stays visible until the new image is placed"
    );

    out.clear();
    *overlay.frame.lock().expect("overlay frame lock") = None;
    overlay.flush(&mut out).expect("clear on buffering");
    assert_eq!(out, b"\x1b_Ga=d,d=I,i=43,q=2\x1b\\");
    out.clear();
    overlay.flush(&mut out).expect("still buffering");
    assert!(out.is_empty());
}

#[test]
fn kitty_file_upload_exposes_the_rgb_bytes_and_deletes_the_image_when_hidden() {
    let directory = tempfile::tempdir().expect("private temporary frame directory");
    let overlay = overlay(Some(directory));
    overlay.active.store(true, Ordering::Relaxed);
    *overlay.rect.lock().expect("overlay bounds lock") = Some(Rect::new(0, 0, 2, 1));
    *overlay.frame.lock().expect("overlay frame lock") = Some(Arc::new(RgbFrame {
        width: 2,
        height: 1,
        data: vec![1, 2, 3, 4, 5, 6],
        seq: 1,
    }));

    let mut out = Vec::new();
    overlay.flush(&mut out).expect("file-based Kitty upload");
    let output = std::str::from_utf8(&out).expect("Kitty command text");
    let upload = output.split_once("\x1b_G").expect("upload command").1;
    let (options, path) = upload.split_once(';').expect("path payload");
    assert!(options.starts_with("a=T,t=f,f=24,s=2,v=1,c=2,r=1,i=42,"));
    let encoded = path.split_once("\x1b\\").expect("terminated upload").0;
    let path = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .expect("path encoding");
    let path = String::from_utf8(path).expect("path text");
    assert_eq!(fs::read(path).expect("frame file"), [1, 2, 3, 4, 5, 6]);

    out.clear();
    overlay.active.store(false, Ordering::Relaxed);
    overlay.flush(&mut out).expect("hide image");
    assert_eq!(out, b"\x1b_Ga=d,d=I,i=42,q=2\x1b\\");
}

#[cfg(unix)]
mod processes;
