//! Bounded projection of large Wiktextract records. Most dictionary records
//! keep the existing Value parser; large records discard unrelated metadata
//! while deserializing instead of building its complete object/array tree.
use serde::{
    de::{IgnoredAny, SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::{Map, Value};
use std::fmt;

pub(super) const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const SMALL_LINE_BYTES: usize = 1024 * 1024;

#[derive(Default)]
struct Strings<const LIMIT: usize, const NONEMPTY: bool>(Vec<String>);

impl<'de, const LIMIT: usize, const NONEMPTY: bool> Deserialize<'de> for Strings<LIMIT, NONEMPTY> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ListVisitor<const LIMIT: usize, const NONEMPTY: bool>;
        impl<'de, const LIMIT: usize, const NONEMPTY: bool> Visitor<'de> for ListVisitor<LIMIT, NONEMPTY> {
            type Value = Strings<LIMIT, NONEMPTY>;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an array of dictionary strings")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while values.len() < LIMIT {
                    let Some(value) = seq.next_element::<String>()? else {
                        return Ok(Strings(values));
                    };
                    if !NONEMPTY || !value.trim().is_empty() {
                        values.push(value);
                    }
                }
                // Still validate the rest of the JSON, without retaining it.
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Strings(values))
            }
        }
        deserializer.deserialize_seq(ListVisitor::<LIMIT, NONEMPTY>)
    }
}

#[derive(Default)]
struct SenseGlosses(Vec<String>);

impl<'de> Deserialize<'de> for SenseGlosses {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Sense {
            #[serde(default)]
            glosses: Strings<40, true>,
        }
        struct SensesVisitor;
        impl<'de> Visitor<'de> for SensesVisitor {
            type Value = SenseGlosses;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an array of dictionary senses")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while values.len() < 40 {
                    let Some(sense) = seq.next_element::<Sense>()? else {
                        return Ok(SenseGlosses(values));
                    };
                    let remaining = 40 - values.len();
                    values.extend(sense.glosses.0.into_iter().take(remaining));
                }
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(SenseGlosses(values))
            }
        }
        deserializer.deserialize_seq(SensesVisitor)
    }
}

#[derive(Default)]
struct Forms(Vec<String>);

impl<'de> Deserialize<'de> for Forms {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Form {
            form: Option<String>,
        }
        struct FormsVisitor;
        impl<'de> Visitor<'de> for FormsVisitor {
            type Value = Forms;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an array of dictionary forms")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while values.len() < 300 {
                    let Some(form) = seq.next_element::<Form>()? else {
                        return Ok(Forms(values));
                    };
                    if let Some(value) = form.form {
                        values.push(value);
                    }
                }
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Forms(values))
            }
        }
        deserializer.deserialize_seq(FormsVisitor)
    }
}

#[derive(Deserialize)]
struct LargeRecord {
    term: Option<String>,
    word: Option<String>,
    lang_code: Option<String>,
    #[serde(rename = "partOfSpeech")]
    part_of_speech: Option<String>,
    pos: Option<String>,
    #[serde(default)]
    definitions: Strings<40, true>,
    #[serde(default)]
    senses: SenseGlosses,
    #[serde(default)]
    examples: Strings<4, false>,
    #[serde(default)]
    aliases: Strings<300, false>,
    #[serde(default)]
    forms: Forms,
}

fn strings(values: impl Iterator<Item = String>) -> Value {
    Value::Array(values.map(Value::String).collect())
}

pub(super) fn parse(line: &str) -> Result<Value, serde_json::Error> {
    if line.len() <= SMALL_LINE_BYTES {
        return serde_json::from_str(line);
    }
    let record: LargeRecord = serde_json::from_str(line)?;
    let mut fields = Map::new();
    for (key, value) in [
        ("term", record.term),
        ("word", record.word),
        ("lang_code", record.lang_code),
        ("partOfSpeech", record.part_of_speech),
        ("pos", record.pos),
    ] {
        if let Some(value) = value {
            fields.insert(key.into(), Value::String(value));
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
