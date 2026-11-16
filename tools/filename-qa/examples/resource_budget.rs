//! Explicit aggregate-only resource probe. The controller applies limits before G.
#[allow(dead_code)]
#[path = "../../../src-tauri/src/resource_policy.rs"]
mod resource_policy;
fn main() {
    use std::io::{Read, Write};
    if std::env::var("PRISM_DEV_RESOURCE_BUDGET").as_deref() != Ok("1") {
        eprintln!("Opt-in resource diagnostic is disabled.");
        std::process::exit(78);
    }
    println!("{{\"waitingForStart\":true}}");
    std::io::stdout().flush().unwrap();
    let mut start = [0];
    std::io::stdin().read_exact(&mut start).unwrap();
    assert_eq!(start, *b"G");
    let budget = resource_policy::WorkBudget::current();
    println!(
        "{{\"budget\":{},\"cpuThreads\":{},\"inferenceThreads\":{},\"ioThreads\":{}}}",
        serde_json::to_string(&budget).unwrap(),
        budget.cpu_threads(),
        budget.inference_threads(),
        budget.io_threads()
    );
    std::io::stdout().flush().unwrap();
    let mut done = [0];
    std::io::stdin().read_exact(&mut done).unwrap();
    assert_eq!(done, *b"Q");
}
