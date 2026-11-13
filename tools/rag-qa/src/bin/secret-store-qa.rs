//! Explicit integration probe for a disposable Linux Secret Service session.
//! It cannot select the inherited login bus or accept real credential values.
#[cfg(target_os = "linux")]
#[allow(dead_code)]
#[path = "../../../../src-tauri/src/secret_store.rs"]
mod secret_store;

#[cfg(target_os = "linux")]
fn run() -> Result<(), &'static str> {
    use secret_store::{SecretErrorCode, SecretStore, SecretValue};
    use std::{os::unix::fs::FileTypeExt, path::PathBuf};

    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2
        || !matches!(
            args[0].as_str(),
            "prepare" | "unavailable" | "verify" | "remove"
        )
        || args[1].len() != 32
        || !args[1].bytes().all(|byte| byte.is_ascii_hexdigit())
        || std::env::var("PRISM_TEST_OS_CREDENTIALS").as_deref() != Ok("1")
    {
        return Err("Requires an explicit disposable-session phase and nonce.");
    }
    let runtime = PathBuf::from(
        std::env::var_os("PRISM_TEST_SECRET_RUNTIME").ok_or("Missing private runtime.")?,
    );
    let root = runtime.parent().ok_or("Invalid private runtime.")?;
    if !runtime.is_absolute()
        || runtime.file_name().and_then(|name| name.to_str()) != Some("runtime")
        || !root
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("prism-secret-service-"))
        || !std::fs::symlink_metadata(root).is_ok_and(|meta| meta.is_dir())
        || !std::fs::symlink_metadata(&runtime).is_ok_and(|meta| meta.is_dir())
        || std::fs::read_to_string(root.join("nonce")).ok().as_deref() != Some(args[1].as_str())
        || std::env::var("DBUS_SESSION_BUS_ADDRESS").as_deref()
            != Ok(format!("unix:path={}", runtime.join("bus").display()).as_str())
        || !std::fs::symlink_metadata(runtime.join("bus"))
            .is_ok_and(|meta| meta.file_type().is_socket())
    {
        return Err("Refusing a session without its owned private bus and nonce.");
    }
    // The Python controller also checks the bus and service peer PIDs before
    // every phase. These fixture checks are not a general-purpose sandbox.
    let store = SecretStore::new(root.join("unused-profile"));
    let id = format!("qa_{}", args[1]);
    let other = format!("qa_other_{}", args[1]);
    let value = |text: &str| SecretValue::new(text.to_owned()).map_err(|_| "Invalid fixture.");
    let matches = |expected: &str| -> Result<(), &'static str> {
        let actual = store.get(&id).map_err(|_| "Credential read failed.")?;
        if actual.as_ref().map(SecretValue::expose) != Some(expected) {
            return Err("Credential roundtrip or preservation failed.");
        }
        Ok(())
    };
    match args[0].as_str() {
        "prepare" => {
            if store
                .get(&id)
                .map_err(|_| "Initial read failed.")?
                .is_some()
            {
                return Err("Disposable credential already exists.");
            }
            store
                .set(&id, &value("synthetic-first-key")?)
                .map_err(|_| "First write failed.")?;
            matches("synthetic-first-key")?;
            if store
                .get(&other)
                .map_err(|_| "Isolation read failed.")?
                .is_some()
            {
                return Err("Credential identities are not isolated.");
            }
            store
                .set(&id, &value("synthetic-second-key")?)
                .map_err(|_| "Replacement failed.")?;
            matches("synthetic-second-key")?;
        }
        "unavailable" => {
            let errors = [
                store.get(&id).map(|_| ()),
                store.set(&id, &value("synthetic-must-not-replace")?),
                store.remove(&id),
            ];
            for result in errors {
                let error = result
                    .err()
                    .ok_or("Unavailable storage unexpectedly succeeded.")?;
                if error.code != SecretErrorCode::Storage
                    || !error.message.contains("preserved")
                    || error.message.contains(&id)
                {
                    return Err("Storage failure did not preserve the public error contract.");
                }
            }
        }
        "verify" => matches("synthetic-second-key")?,
        "remove" => {
            store.remove(&id).map_err(|_| "Removal failed.")?;
            store.remove(&id).map_err(|_| "Repeated removal failed.")?;
            if store.get(&id).map_err(|_| "Final read failed.")?.is_some() {
                return Err("Credential remained after removal.");
            }
        }
        _ => unreachable!(),
    }
    println!("{}", serde_json::json!({"passed": true, "phase": args[0]}));
    Ok(())
}

fn main() {
    #[cfg(target_os = "linux")]
    let result = run();
    #[cfg(not(target_os = "linux"))]
    let result: Result<(), &str> = Err("This disposable Secret Service probe requires Linux.");
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
