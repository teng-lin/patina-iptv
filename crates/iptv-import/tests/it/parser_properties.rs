//! Generated, synthetic invariants at the same entry points used by imports.
use iptv_import::backup_manifest;
use iptv_providers::xtream::{CatalogRow, ListKind, XtreamListParser};
use iptv_providers::{HttpResponse, model::ContentKind};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn m3u_chunking_preserves_identity(names in prop::collection::vec("[A-Za-z0-9 ]{1,40}",0..40),chunk in 1usize..80) {
        let mut text=String::from("#EXTM3U\r\n");
        for (i,name) in names.iter().enumerate(){text.push_str(&format!("#EXTINF:-1 tvg-id=\"synthetic-{i}\",{name}\r\nhttps://media.example.test/{i}.ts\r\n"));}
        let expected=iptv_providers::m3u::parse_m3u(&text);
        let mut p=iptv_providers::m3u::M3uParser::new();for bytes in text.as_bytes().chunks(chunk){p.feed(bytes);}let actual=p.finish();
        prop_assert_eq!(actual.channels,expected.channels);prop_assert_eq!(actual.error,expected.error);
    }
    #[test]
    fn xtream_chunking_matches_buffered_typed_lists(ids in prop::collection::vec(1u32..100000,0..60),chunk in 1usize..101) {
        let rows:Vec<_>=ids.iter().map(|id|serde_json::json!({"stream_id":id,"name":format!("Synthetic {id}"),"category_id":"1","unknown":{"nested":[1,2,3]}})).collect();
        let bytes=serde_json::to_vec(&rows).unwrap();let response=HttpResponse::new(200,&bytes[..]);
        let expected=iptv_providers::xtream::parse_live_streams(&response).unwrap();
        let mut p=XtreamListParser::new(ListKind::Live,Default::default());let mut out=Vec::new();
        let mut emit=|row|if let CatalogRow::Channel(c)=row{out.push(c)};
        for c in bytes.chunks(chunk){p.feed(c,&mut emit).unwrap();}p.finish(&mut emit).unwrap();
        prop_assert_eq!(out,expected.items);
    }
    #[test]
    fn stalker_rows_keep_stable_ids_across_envelope_shapes(ids in prop::collection::vec(1u32..100000,0..60)) {
        let rows:Vec<_>=ids.iter().map(|id|serde_json::json!({"id":id,"name":format!("Synthetic {id}"),"cmd":format!("http://media.example.test/{id}.ts")})).collect();
        let a=HttpResponse::new(200,serde_json::to_vec(&serde_json::json!({"js":{"data":rows}})).unwrap());
        let b=HttpResponse::new(200,serde_json::to_vec(&serde_json::json!({"js":rows})).unwrap());
        prop_assert_eq!(iptv_providers::stalker::parse_channel_page(&a).unwrap().items.items,iptv_providers::stalker::parse_channel_page(&b).unwrap().items.items);
    }
    #[test]
    fn xmltv_and_gzip_chunking_preserve_programmes(count in 0usize..40,chunk in 1usize..89) {
        let mut xml=String::from("<tv><channel id=\"s\"><display-name>Synthetic</display-name></channel>");
        for i in 0..count {
            xml.push_str(&format!(
                "<programme channel=\"s\" start=\"20261112010000 +0000\" stop=\"20261112020000 +0000\"><title>Synthetic {i}</title></programme>"
            ));
        }
        xml.push_str("</tv>");
        let expected=iptv_epg::parse_xmltv(xml.as_bytes(),Default::default()).unwrap();
        use std::io::Write;let mut encoder=flate2::write::GzEncoder::new(Vec::new(),Default::default());encoder.write_all(xml.as_bytes()).unwrap();let compressed=encoder.finish().unwrap();
        let mut parser = iptv_epg::XmltvParser::new(Default::default());
        let mut out = Vec::new();
        for c in compressed.chunks(chunk) {
            parser.feed(c, &mut |i| out.push(i)).unwrap();
        }
        parser.finish(&mut |i| out.push(i)).unwrap();
        prop_assert_eq!(out.len(),expected.channels.len()+expected.programmes.len());
        prop_assert_eq!(iptv_epg::parse_xmltv(&compressed,Default::default()).unwrap().programmes,expected.programmes);
    }
    #[test]
    fn catchup_unknown_templates_are_preserved(s in "[a-zA-Z0-9_/.-]{0,100}",now in any::<i64>()) {
        let template=format!("https://media.example.test/{s}/{{unknown}}/{{utc}}/{{now}}");
        let result=iptv_providers::m3u::expand_live_url(&template,now,0);
        prop_assert!(result.contains("{unknown}"), "unknown placeholder changed");prop_assert!(result.contains(&now.to_string()));
    }
    #[test]
    fn unsafe_backup_entries_never_validate(tail in "[a-z]{1,30}",prefix in prop::sample::select(vec!["../","/","playlists/../","playlists/./","playlists//","playlists/\\","playlists/C:"])) {
        let name=format!("{prefix}{tail}");prop_assert!(!backup_manifest::allowed(&name));
        let bytes = serde_json::to_vec(&serde_json::json!({
            "format": "patina-whole-app",
            "version": 1,
            "root": "/synthetic",
            "files": [
                {"name": "iptv.sqlite", "length": 0},
                {"name": "settings.json", "length": 0},
                {"name": name, "length": 0}
            ]
        }))
        .unwrap();
        prop_assert!(backup_manifest::parse(&bytes).is_err());
    }
    #[test]
    fn manifest_declared_sizes_are_bounded(size in any::<u64>()) {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "format": "patina-whole-app",
            "version": 1,
            "root": "/synthetic",
            "files": [
                {"name": "iptv.sqlite", "length": 0},
                {"name": "settings.json", "length": size}
            ]
        }))
        .unwrap();
        prop_assert_eq!(backup_manifest::parse(&bytes).is_ok(),size<=1<<20);
    }
    #[test]
    fn arbitrary_utf8_never_panics(text in any::<String>()) {
        let _=iptv_providers::content_url::local_path(&text);let _=backup_manifest::parse(text.as_bytes());
        let response=HttpResponse::new(200,text.as_bytes());let _=iptv_providers::xtream::parse_categories(&response,ContentKind::Live);let _=iptv_providers::stalker::parse_channel_page(&response);
    }
}
