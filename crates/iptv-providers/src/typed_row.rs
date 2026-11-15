//! Typed catalog rows. Unknown/nested fields are skipped by serde rather than
//! retained in a JSON tree; lenient scalar rules match the legacy detail parsers.
use crate::lenient::{Obj, list_items};
use serde::{
    Deserialize, Deserializer,
    de::{self, IgnoredAny, MapAccess, SeqAccess, Visitor},
};
use std::fmt;

#[derive(Debug, Default)]
pub(crate) enum Scalar {
    #[default]
    Null,
    Text(String),
    Number(serde_json::Number),
    Bool(bool),
}
impl<'de> Deserialize<'de> for Scalar {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Scalar;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a scalar")
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<Scalar, E> {
                Ok(Scalar::Text(s.into()))
            }
            fn visit_string<E: de::Error>(self, s: String) -> Result<Scalar, E> {
                Ok(Scalar::Text(s))
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Scalar, E> {
                Ok(Scalar::Bool(v))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Scalar, E> {
                Ok(Scalar::Number(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Scalar, E> {
                Ok(Scalar::Number(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Scalar, E> {
                Ok(serde_json::Number::from_f64(v).map_or(Scalar::Null, Scalar::Number))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Scalar, E> {
                Ok(Scalar::Null)
            }
            fn visit_none<E: de::Error>(self) -> Result<Scalar, E> {
                Ok(Scalar::Null)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Scalar, A::Error> {
                while a.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Scalar::Null)
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Scalar, A::Error> {
                while a.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(Scalar::Null)
            }
        }
        d.deserialize_any(V)
    }
}
impl Scalar {
    pub fn string(&self) -> Option<String> {
        match self {
            Self::Text(s) => (!s.trim().is_empty()).then(|| s.trim().to_owned()),
            Self::Number(n) => Some(n.to_string()),
            Self::Bool(v) => Some(v.to_string()),
            Self::Null => None,
        }
    }
    fn number(&self) -> Option<f64> {
        let n = match self {
            Self::Text(s) => s.trim().replace(',', ".").parse().ok(),
            Self::Number(n) => n.as_f64(),
            _ => None,
        }?;
        n.is_finite().then_some(n)
    }
    fn integer(&self) -> Option<i64> {
        match self {
            Self::Text(s) => s.trim().parse().ok(),
            Self::Number(n) => n.as_i64(),
            _ => None,
        }
        .or_else(|| {
            self.number()
                .filter(|f| f.fract() == 0.0 && f.abs() < 9e15)
                .map(|f| f as i64)
        })
    }
    fn boolean(&self) -> bool {
        match self {
            Self::Bool(v) => *v,
            Self::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
            Self::Text(s) => matches!(
                s.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on" | "y"
            ),
            _ => false,
        }
    }
}

/// Only the columns the catalog consumes are retained.
#[derive(Debug, Default)]
#[allow(non_snake_case)]
pub(crate) struct Row {
    stream_id: Scalar,
    is_adult: Scalar,
    series_id: Scalar,
    id: Scalar,
    name: Scalar,
    title: Scalar,
    o_name: Scalar,
    num: Scalar,
    number: Scalar,
    stream_icon: Scalar,
    epg_channel_id: Scalar,
    stream_type: Scalar,
    category_id: Scalar,
    category_name: Scalar,
    tv_archive: Scalar,
    tv_archive_duration: Scalar,
    direct_source: Scalar,
    container_extension: Scalar,
    movie_image: Scalar,
    cover_big: Scalar,
    cover: Scalar,
    plot: Scalar,
    description: Scalar,
    year: Scalar,
    releaseDate: Scalar,
    release_date: Scalar,
    releasedate: Scalar,
    rating: Scalar,
    rating_5based: Scalar,
    added: Scalar,
    duration_secs: Scalar,
    duration: Scalar,
    last_modified: Scalar,
    logo: Scalar,
    tv_genre_id: Scalar,
    xmltv_id: Scalar,
    cmd: Scalar,
    use_http_tmp_link: Scalar,
    use_load_balancing: Scalar,
    is_series: Scalar,
    series: Vec<String>,
    rating_imdb: Scalar,
    rating_kinopoisk: Scalar,
    screenshot_uri: Scalar,
    time: Scalar,
    censored: Scalar,
    alias: Scalar,
    url: Scalar,
    category_ids: Vec<String>,
    cmds: Vec<Command>,
}
#[derive(Debug, Default)]
struct Command {
    url: Scalar,
    cmd: Scalar,
}
// Unescaped JSON keys borrow directly from the input row; unusual escaped
// keys allocate only their decoded spelling. No per-field key allocation.
struct Key<'a>(std::borrow::Cow<'a, str>);
impl<'de> Deserialize<'de> for Key<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Key<'de>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a field name")
            }
            fn visit_borrowed_str<E: de::Error>(self, s: &'de str) -> Result<Self::Value, E> {
                Ok(Key(std::borrow::Cow::Borrowed(s)))
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
                Ok(Key(std::borrow::Cow::Owned(s.into())))
            }
        }
        d.deserialize_identifier(V)
    }
}

// Match the legacy map accessors: last duplicate key wins, unknown fields
// and malformed optional command values are ignored without a JSON tree.
struct StringList(Vec<String>);
impl<'de> Deserialize<'de> for StringList {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        strings(d).map(Self)
    }
}
struct Commands(Vec<Command>);
impl<'de> Deserialize<'de> for Commands {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        commands(d).map(Self)
    }
}
impl<'de> Deserialize<'de> for Row {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Row;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a catalog object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Row, A::Error> {
                let mut row = Row::default();
                while let Some(key) = a.next_key::<Key<'de>>()? {
                    match key.0.as_ref() {
                        "stream_id" => row.stream_id = a.next_value()?,
                        "is_adult" => row.is_adult = a.next_value()?,
                        "series_id" => row.series_id = a.next_value()?,
                        "id" => row.id = a.next_value()?,
                        "name" => row.name = a.next_value()?,
                        "title" => row.title = a.next_value()?,
                        "o_name" => row.o_name = a.next_value()?,
                        "num" => row.num = a.next_value()?,
                        "number" => row.number = a.next_value()?,
                        "stream_icon" => row.stream_icon = a.next_value()?,
                        "epg_channel_id" => row.epg_channel_id = a.next_value()?,
                        "stream_type" => row.stream_type = a.next_value()?,
                        "category_id" => row.category_id = a.next_value()?,
                        "category_name" => row.category_name = a.next_value()?,
                        "tv_archive" => row.tv_archive = a.next_value()?,
                        "tv_archive_duration" => row.tv_archive_duration = a.next_value()?,
                        "direct_source" => row.direct_source = a.next_value()?,
                        "container_extension" => row.container_extension = a.next_value()?,
                        "movie_image" => row.movie_image = a.next_value()?,
                        "cover_big" => row.cover_big = a.next_value()?,
                        "cover" => row.cover = a.next_value()?,
                        "plot" => row.plot = a.next_value()?,
                        "description" => row.description = a.next_value()?,
                        "year" => row.year = a.next_value()?,
                        "releaseDate" => row.releaseDate = a.next_value()?,
                        "release_date" => row.release_date = a.next_value()?,
                        "releasedate" => row.releasedate = a.next_value()?,
                        "rating" => row.rating = a.next_value()?,
                        "rating_5based" => row.rating_5based = a.next_value()?,
                        "added" => row.added = a.next_value()?,
                        "duration_secs" => row.duration_secs = a.next_value()?,
                        "duration" => row.duration = a.next_value()?,
                        "last_modified" => row.last_modified = a.next_value()?,
                        "logo" => row.logo = a.next_value()?,
                        "tv_genre_id" => row.tv_genre_id = a.next_value()?,
                        "xmltv_id" => row.xmltv_id = a.next_value()?,
                        "cmd" => row.cmd = a.next_value()?,
                        "use_http_tmp_link" => row.use_http_tmp_link = a.next_value()?,
                        "use_load_balancing" => row.use_load_balancing = a.next_value()?,
                        "is_series" => row.is_series = a.next_value()?,
                        "series" => row.series = a.next_value::<StringList>()?.0,
                        "rating_imdb" => row.rating_imdb = a.next_value()?,
                        "rating_kinopoisk" => row.rating_kinopoisk = a.next_value()?,
                        "screenshot_uri" => row.screenshot_uri = a.next_value()?,
                        "time" => row.time = a.next_value()?,
                        "censored" => row.censored = a.next_value()?,
                        "alias" => row.alias = a.next_value()?,
                        "url" => row.url = a.next_value()?,
                        "category_ids" => row.category_ids = a.next_value::<StringList>()?.0,
                        "cmds" => row.cmds = a.next_value::<Commands>()?.0,
                        _ => {
                            let _: IgnoredAny = a.next_value()?;
                        }
                    }
                }
                Ok(row)
            }
        }
        d.deserialize_map(V)
    }
}
impl<'de> Deserialize<'de> for Command {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Command;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an optional command")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Command, A::Error> {
                let mut c = Command::default();
                while let Some(k) = a.next_key::<Key<'de>>()? {
                    match k.0.as_ref() {
                        "url" => c.url = a.next_value()?,
                        "cmd" => c.cmd = a.next_value()?,
                        _ => {
                            let _: IgnoredAny = a.next_value()?;
                        }
                    }
                }
                Ok(c)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Command, A::Error> {
                while a.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Command::default())
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Command, E> {
                Ok(Command::default())
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Command, E> {
                Ok(Command::default())
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Command, E> {
                Ok(Command::default())
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Command, E> {
                Ok(Command::default())
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Command, E> {
                Ok(Command::default())
            }
            fn visit_unit<E: de::Error>(self) -> Result<Command, E> {
                Ok(Command::default())
            }
        }
        d.deserialize_any(V)
    }
}
fn strings<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = Vec<String>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("strings")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
            let mut v = Vec::new();
            while let Some(s) = a.next_element::<Scalar>()? {
                if let Some(s) = s.string() {
                    v.push(s);
                }
            }
            Ok(v)
        }
        fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
            Ok(s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect())
        }
        fn visit_u64<E: de::Error>(self, n: u64) -> Result<Self::Value, E> {
            Ok(vec![n.to_string()])
        }
        fn visit_i64<E: de::Error>(self, n: i64) -> Result<Self::Value, E> {
            Ok(vec![n.to_string()])
        }
        fn visit_f64<E: de::Error>(self, n: f64) -> Result<Self::Value, E> {
            Ok(vec![n.to_string()])
        }
        fn visit_bool<E: de::Error>(self, n: bool) -> Result<Self::Value, E> {
            Ok(vec![n.to_string()])
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
            while a.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
            Ok(Vec::new())
        }
    }
    d.deserialize_any(V)
}
fn commands<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Command>, D::Error> {
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = Vec<Command>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("commands")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
            let mut v = Vec::new();
            while let Some(c) = a.next_element::<Command>()? {
                v.push(c);
            }
            Ok(v)
        }
        fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
            let mut v = Vec::new();
            while let Some((_, c)) = a.next_entry::<IgnoredAny, Command>()? {
                v.push(c);
            }
            Ok(v)
        }
        fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
    }
    d.deserialize_any(V)
}

pub(crate) trait Fields: Copy {
    fn str(&self, key: &str) -> Option<String>;
    fn u64(&self, key: &str) -> Option<u64>;
    fn i64(&self, key: &str) -> Option<i64>;
    fn f64(&self, key: &str) -> Option<f64>;
    fn flag(&self, key: &str) -> bool;
    fn has(&self, key: &str) -> bool;
    fn strings(&self, key: &str) -> Vec<String>;
    fn commands(&self) -> Option<String>;
    fn series_flag(&self) -> bool;
    fn u32(&self, key: &str) -> Option<u32> {
        self.u64(key).and_then(|v| v.try_into().ok())
    }
    fn text(&self, key: &str) -> Option<String> {
        self.str(key).filter(|s| {
            !["n/a", "na", "-", "--", "null", "none", "undefined"]
                .contains(&s.to_ascii_lowercase().as_str())
        })
    }
    fn first_text(&self, keys: &[&str]) -> Option<String> {
        keys.iter().find_map(|k| self.text(k))
    }
}
impl Row {
    fn get(&self, key: &str) -> Option<&Scalar> {
        Some(match key {
            "is_adult" => &self.is_adult,
            "stream_id" => &self.stream_id,
            "series_id" => &self.series_id,
            "id" => &self.id,
            "name" => &self.name,
            "title" => &self.title,
            "o_name" => &self.o_name,
            "num" => &self.num,
            "number" => &self.number,
            "stream_icon" => &self.stream_icon,
            "epg_channel_id" => &self.epg_channel_id,
            "stream_type" => &self.stream_type,
            "category_id" => &self.category_id,
            "category_name" => &self.category_name,
            "tv_archive" => &self.tv_archive,
            "tv_archive_duration" => &self.tv_archive_duration,
            "direct_source" => &self.direct_source,
            "container_extension" => &self.container_extension,
            "movie_image" => &self.movie_image,
            "cover_big" => &self.cover_big,
            "cover" => &self.cover,
            "plot" => &self.plot,
            "description" => &self.description,
            "year" => &self.year,
            "releaseDate" => &self.releaseDate,
            "release_date" => &self.release_date,
            "releasedate" => &self.releasedate,
            "rating" => &self.rating,
            "rating_5based" => &self.rating_5based,
            "added" => &self.added,
            "duration_secs" => &self.duration_secs,
            "duration" => &self.duration,
            "last_modified" => &self.last_modified,
            "logo" => &self.logo,
            "tv_genre_id" => &self.tv_genre_id,
            "xmltv_id" => &self.xmltv_id,
            "cmd" => &self.cmd,
            "use_http_tmp_link" => &self.use_http_tmp_link,
            "use_load_balancing" => &self.use_load_balancing,
            "is_series" => &self.is_series,
            "rating_imdb" => &self.rating_imdb,
            "rating_kinopoisk" => &self.rating_kinopoisk,
            "screenshot_uri" => &self.screenshot_uri,
            "time" => &self.time,
            "censored" => &self.censored,
            "alias" => &self.alias,
            "url" => &self.url,
            _ => return None,
        })
    }
}
impl Fields for &Row {
    fn str(&self, k: &str) -> Option<String> {
        self.get(k)?.string()
    }
    fn i64(&self, k: &str) -> Option<i64> {
        self.get(k)?.integer()
    }
    fn u64(&self, k: &str) -> Option<u64> {
        self.i64(k)?.try_into().ok()
    }
    fn f64(&self, k: &str) -> Option<f64> {
        self.get(k)?.number()
    }
    fn flag(&self, k: &str) -> bool {
        self.get(k).is_some_and(Scalar::boolean)
    }
    fn has(&self, k: &str) -> bool {
        self.get(k).is_some_and(|v| !matches!(v, Scalar::Null))
    }
    fn strings(&self, k: &str) -> Vec<String> {
        if k == "series" {
            self.series.clone()
        } else if k == "category_ids" {
            self.category_ids.clone()
        } else {
            self.str(k)
                .map(|s| {
                    s.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        }
    }
    fn commands(&self) -> Option<String> {
        self.cmds
            .iter()
            .find_map(|c| c.url.string().or_else(|| c.cmd.string()))
    }
    fn series_flag(&self) -> bool {
        matches!(self.is_series, Scalar::Bool(true))
            || self.str("is_series").as_deref() == Some("1")
    }
}
impl Fields for Obj<'_> {
    fn str(&self, k: &str) -> Option<String> {
        Obj::str(self, k)
    }
    fn i64(&self, k: &str) -> Option<i64> {
        Obj::i64(self, k)
    }
    fn u64(&self, k: &str) -> Option<u64> {
        Obj::u64(self, k)
    }
    fn f64(&self, k: &str) -> Option<f64> {
        Obj::f64(self, k)
    }
    fn flag(&self, k: &str) -> bool {
        Obj::flag(self, k)
    }
    fn has(&self, k: &str) -> bool {
        Obj::has(self, k)
    }
    fn strings(&self, k: &str) -> Vec<String> {
        Obj::strings(self, k)
    }
    fn commands(&self) -> Option<String> {
        self.raw("cmds")
            .and_then(list_items)?
            .into_iter()
            .filter_map(Obj::of)
            .find_map(|c| c.text("url").or_else(|| c.text("cmd")))
    }
    fn series_flag(&self) -> bool {
        self.raw("is_series").is_some_and(|v| {
            matches!(v, serde_json::Value::Bool(true))
                || v.as_u64() == Some(1)
                || v.as_str() == Some("1")
        })
    }
}
