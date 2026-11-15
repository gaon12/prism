//! Bounded projection of large dictionary records. Small records keep the
//! existing Value parser. Large records validate but discard unused JSON shapes
//! without constructing their complete object/array trees.
use serde::{
    de::{IgnoredAny, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::{Map, Value};
use std::{fmt, marker::PhantomData};

pub(super) const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const SMALL_LINE_BYTES: usize = 1024 * 1024;

/// Each projection reads its relevant JSON shape. Other shapes have the same
/// meaning as Value::as_str/as_array/get returning None in the small parser.
trait Projection: Default {
    fn ignored() -> Self {
        Self::default()
    }
    fn text(_: &str) -> Self {
        Self::ignored()
    }
    fn sequence<'de, A: SeqAccess<'de>>(mut seq: A) -> Result<Self, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Self::ignored())
    }
    fn object<'de, A: MapAccess<'de>>(mut map: A) -> Result<Self, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Self::ignored())
    }
}

fn project<'de, D: Deserializer<'de>, T: Projection>(deserializer: D) -> Result<T, D::Error> {
    struct ProjectionVisitor<T>(PhantomData<T>);
    impl<'de, T: Projection> Visitor<'de> for ProjectionVisitor<T> {
        type Value = T;
        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a dictionary field")
        }
        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<T, E> {
            Ok(T::text(value))
        }
        fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<T, E> {
            Ok(T::ignored())
        }
        fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<T, E> {
            Ok(T::ignored())
        }
        fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<T, E> {
            Ok(T::ignored())
        }
        fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<T, E> {
            Ok(T::ignored())
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<T, E> {
            Ok(T::ignored())
        }
        fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<T, A::Error> {
            T::sequence(seq)
        }
        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<T, A::Error> {
            T::object(map)
        }
    }
    deserializer.deserialize_any(ProjectionVisitor::<T>(PhantomData))
}

macro_rules! deserialize_projection {
    ($($ty:ty),+ $(,)?) => {$(
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                project(deserializer)
            }
        }
    )+};
}

/// A present non-string must remain present: a null term blocks fallback to
/// word, whereas a missing term permits it. Language/POS fields behave likewise.
#[derive(Default)]
struct Scalar {
    present: bool,
    text: Option<String>,
}
impl Projection for Scalar {
    fn ignored() -> Self {
        Self {
            present: true,
            text: None,
        }
    }
    fn text(value: &str) -> Self {
        Self {
            present: true,
            text: Some(value.into()),
        }
    }
}

#[derive(Default)]
struct Strings<const LIMIT: usize, const NONEMPTY: bool>(Vec<String>);
impl<const LIMIT: usize, const NONEMPTY: bool> Projection for Strings<LIMIT, NONEMPTY> {
    fn sequence<'de, A: SeqAccess<'de>>(mut seq: A) -> Result<Self, A::Error> {
        let mut values = Vec::new();
        while values.len() < LIMIT {
            let Some(value) = seq.next_element::<Scalar>()? else {
                return Ok(Self(values));
            };
            if let Some(value) = value.text {
                if !NONEMPTY || !value.trim().is_empty() {
                    values.push(value);
                }
            }
        }
        // Validate all remaining input, retaining neither strings nor objects.
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Self(values))
    }
}
impl<'de, const LIMIT: usize, const NONEMPTY: bool> Deserialize<'de> for Strings<LIMIT, NONEMPTY> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        project(deserializer)
    }
}

#[derive(Default)]
struct GlossField {
    glosses: Strings<40, true>,
}
impl Projection for GlossField {
    fn object<'de, A: MapAccess<'de>>(mut map: A) -> Result<Self, A::Error> {
        let mut value = Self::default();
        while let Some(key) = map.next_key::<String>()? {
            if key == "glosses" {
                value.glosses = map.next_value()?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(value)
    }
}
#[derive(Default)]
struct SenseGlosses(Vec<String>);
impl Projection for SenseGlosses {
    fn sequence<'de, A: SeqAccess<'de>>(mut seq: A) -> Result<Self, A::Error> {
        let mut values = Vec::new();
        while values.len() < 40 {
            let Some(sense) = seq.next_element::<GlossField>()? else {
                return Ok(Self(values));
            };
            let remaining = 40 - values.len();
            values.extend(sense.glosses.0.into_iter().take(remaining));
        }
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Self(values))
    }
}

#[derive(Default)]
struct FormField {
    form: Scalar,
}
impl Projection for FormField {
    fn object<'de, A: MapAccess<'de>>(mut map: A) -> Result<Self, A::Error> {
        let mut value = Self::default();
        while let Some(key) = map.next_key::<String>()? {
            if key == "form" {
                value.form = map.next_value()?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(value)
    }
}
#[derive(Default)]
struct Forms(Vec<String>);
impl Projection for Forms {
    fn sequence<'de, A: SeqAccess<'de>>(mut seq: A) -> Result<Self, A::Error> {
        let mut values = Vec::new();
        while values.len() < 300 {
            let Some(form) = seq.next_element::<FormField>()? else {
                return Ok(Self(values));
            };
            if let Some(value) = form.form.text {
                values.push(value);
            }
        }
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Self(values))
    }
}

#[derive(Default)]
struct LargeRecord {
    header: bool,
    term: Scalar,
    word: Scalar,
    lang_code: Scalar,
    part_of_speech: Scalar,
    pos: Scalar,
    definitions: Strings<40, true>,
    senses: SenseGlosses,
    examples: Strings<4, false>,
    aliases: Strings<300, false>,
    forms: Forms,
}
impl Projection for LargeRecord {
    fn object<'de, A: MapAccess<'de>>(mut map: A) -> Result<Self, A::Error> {
        let mut record = Self::default();
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "prismDictionary" => {
                    map.next_value::<IgnoredAny>()?;
                    record.header = true;
                }
                "term" => record.term = map.next_value()?,
                "word" => record.word = map.next_value()?,
                "lang_code" => record.lang_code = map.next_value()?,
                "partOfSpeech" => record.part_of_speech = map.next_value()?,
                "pos" => record.pos = map.next_value()?,
                "definitions" => record.definitions = map.next_value()?,
                "senses" => record.senses = map.next_value()?,
                "examples" => record.examples = map.next_value()?,
                "aliases" => record.aliases = map.next_value()?,
                "forms" => record.forms = map.next_value()?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(record)
    }
}
deserialize_projection!(
    Scalar,
    GlossField,
    SenseGlosses,
    FormField,
    Forms,
    LargeRecord
);

fn strings(values: impl Iterator<Item = String>) -> Value {
    Value::Array(values.map(Value::String).collect())
}

pub(super) fn parse(line: &str) -> Result<Value, serde_json::Error> {
    if line.len() <= SMALL_LINE_BYTES {
        return serde_json::from_str(line);
    }
    let record: LargeRecord = serde_json::from_str(line)?;
    if record.header {
        return Ok(serde_json::json!({ "prismDictionary": null }));
    }
    let mut fields = Map::new();
    for (key, value) in [
        ("term", record.term),
        ("word", record.word),
        ("lang_code", record.lang_code),
        ("partOfSpeech", record.part_of_speech),
        ("pos", record.pos),
    ] {
        if value.present {
            fields.insert(key.into(), value.text.map_or(Value::Null, Value::String));
        }
    }
    fields.insert(
        "definitions".into(),
        strings(record.definitions.0.into_iter().chain(record.senses.0)),
    );
    fields.insert("examples".into(), strings(record.examples.0.into_iter()));
    fields.insert(
        "aliases".into(),
        strings(record.aliases.0.into_iter().chain(record.forms.0).take(300)),
    );
    Ok(Value::Object(fields))
}
