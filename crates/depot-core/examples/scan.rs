//! Print the inventory as JSON: `cargo run --release -p depot-core --example scan`
fn main() {
    let t = std::time::Instant::now();
    let inv = depot_core::scan(&depot_core::Env::system());
    let elapsed = t.elapsed();
    if std::env::args().any(|a| a == "--summary") {
        for p in inv.packages.iter().filter(|p| p.role == depot_core::Role::App) {
            println!("{:8} {:40} {}", p.kind.badge(), p.title, p.id);
        }
        eprintln!("{} packages, {} warnings, {elapsed:?}", inv.packages.len(), inv.warnings.len());
        for w in &inv.warnings {
            eprintln!("warning: {w}");
        }
    } else {
        println!("{}", serde_json::to_string_pretty(&inv).unwrap());
    }
}
