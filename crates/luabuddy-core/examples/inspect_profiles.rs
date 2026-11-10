use luabuddy_core::read_profiles;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "profiles".to_owned());
    let profiles = read_profiles(Path::new(&directory))?;
    for profile in &profiles {
        if let Some(error) = profile.hook_migration_error() {
            eprintln!("{error}");
        }
    }
    serde_json::to_writer_pretty(std::io::stdout(), &profiles)?;
    println!();
    Ok(())
}
