use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufRead, Read},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use unicode_normalization::UnicodeNormalization;

pub const LANGUAGES: [&str; 4] = ["ko", "en", "zh", "ja"];
fn key(word: &str) -> String {
    word.trim().nfc().collect::<String>().to_lowercase()
}
fn default_gloss_language() -> String {
    "en".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackManifest {
    pub schema_version: u32,
    pub language: String,
    pub name: String,
    pub version: String,
    pub source_url: String,
    pub license: String,
    pub license_url: String,
    #[serde(default = "default_gloss_language")]
    pub definition_language: String,
}
impl PackManifest {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 || !LANGUAGES.contains(&self.language.as_str()) {
            return Err("Unsupported dictionary pack format or language.".into());
        }
        if self.name.trim().is_empty()
            || self.name.len() > 200
            || self.version.len() > 100
            || self.license.trim().is_empty()
            || self.license.len() > 200
            || self.definition_language.len() > 20
        {
            return Err("Dictionary pack metadata is incomplete.".into());
        }
        for url in [&self.source_url, &self.license_url] {
            let parsed = tauri::Url::parse(url)
                .map_err(|_| "Dictionary source and license must be valid web URLs.")?;
            if !["https", "http"].contains(&parsed.scheme())
                || parsed.host_str().is_none()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || url.len() > 2048
            {
                return Err("Invalid dictionary attribution URL.".into());
            }
        }
        Ok(())
    }
    fn id(&self) -> String {
        format!(
            "{:x}",
            Sha256::digest(format!(
                "{}\n{}\n{}",
                self.language, self.name, self.source_url
            ))
        )
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPack {
    pub id: String,
    pub manifest: PackManifest,
    pub entries: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Definition {
    pub term: String,
    pub language: String,
    pub part_of_speech: String,
    pub text: String,
    #[serde(default)]
    pub examples: Vec<String>,
    pub source_url: String,
    pub source_name: String,
    pub license: String,
    pub license_url: String,
    pub definition_language: String,
}
#[derive(Clone)]
pub struct DictionaryStore {
    path: PathBuf,
}
impl DictionaryStore {
    pub fn open(directory: &Path) -> Result<Self, String> {
        fs::create_dir_all(directory).map_err(|_| "Could not create the dictionary directory.")?;
        let store = Self {
            path: directory.join("dictionaries.sqlite3"),
        };
        let connection = store.connect()?;
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS packs (id TEXT PRIMARY KEY, language TEXT NOT NULL, metadata TEXT NOT NULL, entries INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS definitions (id INTEGER PRIMARY KEY, pack_id TEXT NOT NULL REFERENCES packs(id) ON DELETE CASCADE, lookup TEXT NOT NULL, term TEXT NOT NULL, pos TEXT NOT NULL, text TEXT NOT NULL, examples TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS definitions_lookup ON definitions(lookup, pack_id);
            CREATE TABLE IF NOT EXISTS aliases (pack_id TEXT NOT NULL REFERENCES packs(id) ON DELETE CASCADE, lookup TEXT NOT NULL, target TEXT NOT NULL, PRIMARY KEY(pack_id,lookup,target));
            CREATE INDEX IF NOT EXISTS aliases_lookup ON aliases(lookup);").map_err(|_| "Could not open the dictionary database. The existing file was preserved.")?;
        Ok(store)
    }
    fn connect(&self) -> Result<Connection, String> {
        let connection =
            Connection::open(&self.path).map_err(|_| "Dictionary database unavailable.")?;
        connection
            .busy_timeout(Duration::from_secs(3))
            .map_err(|_| "Dictionary database unavailable.")?;
        connection
            .execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(|_| "Dictionary database unavailable.")?;
        Ok(connection)
    }
    pub fn packs(&self) -> Result<Vec<InstalledPack>, String> {
        let connection = self.connect()?;
        let mut statement = connection
            .prepare("SELECT id,metadata,entries FROM packs ORDER BY language,id")
            .map_err(|_| "Dictionary database unavailable.")?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            })
            .map_err(|_| "Could not list dictionary packs.")?;
        rows.map(|row| {
            let (id, metadata, entries) = row.map_err(|_| "Could not read dictionary metadata.")?;
            Ok(InstalledPack {
                id,
                manifest: serde_json::from_str(&metadata)
                    .map_err(|_| "Dictionary metadata is damaged.")?,
                entries,
            })
        })
        .collect()
    }
    pub fn remove(&self, id: &str) -> Result<(), String> {
        self.connect()?
            .execute("DELETE FROM packs WHERE id=?", [id])
            .map_err(|_| "Could not remove the dictionary pack.")?;
        Ok(())
    }
    pub fn lookup(&self, language: &str, word: &str) -> Result<Vec<Definition>, String> {
        let connection = self.connect()?;
        let mut statement = connection.prepare("SELECT d.term,d.pos,d.text,d.examples,p.metadata FROM definitions d JOIN packs p ON p.id=d.pack_id WHERE p.language=?1 AND (d.lookup=?2 OR d.lookup IN (SELECT target FROM aliases WHERE lookup=?2 AND pack_id=d.pack_id)) ORDER BY d.id LIMIT 40").map_err(|_| "Dictionary database unavailable.")?;
        let rows = statement
            .query_map(params![language, key(word)], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(|_| "Could not search the offline dictionary.")?;
        rows.map(|row| {
            let (term, part_of_speech, text, examples, metadata) =
                row.map_err(|_| "Could not read the offline dictionary.")?;
            let manifest: PackManifest =
                serde_json::from_str(&metadata).map_err(|_| "Dictionary metadata is damaged.")?;
            let mut url = tauri::Url::parse("https://en.wiktionary.org/wiki/").unwrap();
            url.path_segments_mut().unwrap().push(&term);
            Ok(Definition {
                term,
                language: language.into(),
                part_of_speech,
                text,
                examples: serde_json::from_str(&examples).unwrap_or_default(),
                source_url: if manifest.source_url.contains("wiktionary")
                    || manifest.source_url.contains("kaikki.org")
                {
                    url.to_string()
                } else {
                    manifest.source_url.clone()
                },
                source_name: manifest.name,
                license: manifest.license,
                license_url: manifest.license_url,
                definition_language: manifest.definition_language,
            })
        })
        .collect()
    }
    /// Stream Prism JSONL or Wiktextract JSONL in one transaction. Cancel/errors retain the old pack.
    pub fn import<R: BufRead>(
        &self,
        mut reader: R,
        manifest: PackManifest,
        cancel: &AtomicBool,
        progress: impl Fn(u64),
    ) -> Result<InstalledPack, String> {
        manifest.validate()?;
        let id = manifest.id();
        let mut connection = self.connect()?;
        let transaction = connection
            .transaction()
            .map_err(|_| "Dictionary database is busy.")?;
        transaction
            .execute("DELETE FROM packs WHERE id=?", [&id])
            .map_err(|_| "Could not replace the dictionary pack.")?;
        transaction
            .execute(
                "INSERT INTO packs(id,language,metadata,entries) VALUES(?,?,?,0)",
                params![
                    id,
                    manifest.language,
                    serde_json::to_string(&manifest).unwrap()
                ],
            )
            .map_err(|_| "Could not create the dictionary pack.")?;
        let mut count = 0u64;
        let mut line = String::new();
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("Dictionary import cancelled.".into());
            }
            line.clear();
            // Bound each record without first allocating an untrusted giant line.
            let read = (&mut reader)
                .take(1_048_577)
                .read_line(&mut line)
                .map_err(|_| "Could not read dictionary data.")?;
            if read == 0 {
                break;
            }
            if read > 1_048_576 {
                return Err("A dictionary record exceeds 1 MiB.".into());
            }
            if line.trim().is_empty() {
                continue;
            }
            let record: serde_json::Value = serde_json::from_str(&line)
                .map_err(|_| "Invalid dictionary JSONL record. No changes were applied.")?;
            if record.get("prismDictionary").is_some() {
                continue;
            }
            if record
                .get("lang_code")
                .and_then(|value| value.as_str())
                .is_some_and(|language| language != manifest.language)
            {
                continue;
            }
            let Some(term) = record
                .get("term")
                .or_else(|| record.get("word"))
                .and_then(|value| value.as_str())
            else {
                return Err("Dictionary record has no headword.".into());
            };
            if term.trim().is_empty() || term.chars().count() > 256 {
                continue;
            }
            let pos = record
                .get("partOfSpeech")
                .or_else(|| record.get("pos"))
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let mut glosses: Vec<&str> = record
                .get("definitions")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
                .filter_map(|value| value.as_str())
                .collect();
            if let Some(senses) = record.get("senses").and_then(|value| value.as_array()) {
                for sense in senses {
                    glosses.extend(
                        sense
                            .get("glosses")
                            .and_then(|value| value.as_array())
                            .into_iter()
                            .flatten()
                            .filter_map(|value| value.as_str()),
                    );
                }
            }
            let examples: Vec<_> = record
                .get("examples")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
                .filter_map(|value| value.as_str())
                .take(4)
                .map(|value| value.chars().take(1500).collect::<String>())
                .collect();
            for text in glosses
                .into_iter()
                .filter(|value| !value.trim().is_empty())
                .take(40)
            {
                transaction.execute("INSERT INTO definitions(pack_id,lookup,term,pos,text,examples) VALUES(?,?,?,?,?,?)", params![id,key(term),term,pos.chars().take(100).collect::<String>(),text.chars().take(8000).collect::<String>(),serde_json::to_string(&examples).unwrap()]).map_err(|_| "Could not store dictionary data (check available disk space).")?;
                count += 1;
            }
            let aliases = record
                .get("aliases")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
                .filter_map(|value| value.as_str())
                .chain(
                    record
                        .get("forms")
                        .and_then(|value| value.as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(|value| value.get("form").and_then(|value| value.as_str())),
                );
            for alias in aliases
                .take(300)
                .filter(|value| value.chars().count() <= 256)
            {
                transaction
                    .execute(
                        "INSERT OR IGNORE INTO aliases(pack_id,lookup,target) VALUES(?,?,?)",
                        params![id, key(alias), key(term)],
                    )
                    .map_err(|_| "Could not store dictionary word forms.")?;
            }
            if count % 1000 == 0 {
                progress(count);
            }
        }
        if count == 0 {
            return Err(
                "No definitions for the selected language were found. The old pack was preserved."
                    .into(),
            );
        }
        transaction
            .execute("UPDATE packs SET entries=? WHERE id=?", params![count, id])
            .map_err(|_| "Could not finalize the dictionary pack.")?;
        if cancel.load(Ordering::Relaxed) {
            return Err("Dictionary import cancelled.".into());
        }
        transaction
            .commit()
            .map_err(|_| "Could not save the dictionary pack.")?;
        progress(count);
        Ok(InstalledPack {
            id,
            manifest,
            entries: count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn manifest() -> PackManifest {
        PackManifest {
            schema_version: 1,
            language: "ko".into(),
            name: "Fixture".into(),
            version: "1".into(),
            source_url: "https://example.org/fixture".into(),
            license: "CC0".into(),
            license_url: "https://creativecommons.org/publicdomain/zero/1.0/".into(),
            definition_language: "ko".into(),
        }
    }
    #[test]
    fn imports_normalized_words_aliases_and_rolls_back_invalid_updates() {
        let directory = tempfile::tempdir().unwrap();
        let store = DictionaryStore::open(directory.path()).unwrap();
        let cancel = AtomicBool::new(false);
        let data = "{\"word\":\"사과\",\"lang_code\":\"ko\",\"pos\":\"noun\",\"senses\":[{\"glosses\":[\"테스트 뜻풀이\"]}],\"forms\":[{\"form\":\"사과들\"}]}\n";
        let pack = store
            .import(data.as_bytes(), manifest(), &cancel, |_| {})
            .unwrap();
        assert_eq!(pack.entries, 1);
        assert_eq!(store.lookup("ko", "사과").unwrap()[0].text, "테스트 뜻풀이");
        assert_eq!(store.lookup("ko", "사과들").unwrap()[0].term, "사과");
        assert!(store.lookup("en", "사과").unwrap().is_empty());
        assert!(store
            .import(b"invalid json\n".as_slice(), manifest(), &cancel, |_| {})
            .is_err());
        assert_eq!(store.lookup("ko", "사과").unwrap().len(), 1);
        cancel.store(true, Ordering::Relaxed);
        assert!(store
            .import(data.as_bytes(), manifest(), &cancel, |_| {})
            .is_err());
        assert_eq!(store.packs().unwrap().len(), 1);
        store.remove(&pack.id).unwrap();
        assert!(store.lookup("ko", "사과").unwrap().is_empty());
    }
}
