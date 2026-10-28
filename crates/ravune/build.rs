//! Lists every sound in `assets/library` (its subfolders are the kit's folders) for the
//! built-in kit: `browser.rs` includes the list. Add a .wav or .flac there and it's in the app.
//! On Windows it also gives the .exe its icon (assets/icon.ico, see scripts/icon.py).

use std::path::Path;

fn main() {
    // the .exe's icon in Explorer and on shortcuts (the window's own comes from icon.png)
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let icon = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icon.ico");
        println!("cargo:rerun-if-changed={}", icon.display());
        winresource::WindowsResource::new().set_icon(&icon.to_string_lossy()).compile().expect("embed the icon (needs the Windows SDK's rc.exe)");
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/library");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    walk(&root, "", &mut files);
    files.sort();
    let mut list = String::from("[\n");
    for rel in &files {
        list += &format!("    ({rel:?}, include_bytes!({:?})),\n", root.join(rel).to_string_lossy());
    }
    list += "]\n";
    let out = Path::new(&std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("builtin.rs");
    std::fs::write(out, list).expect("write the kit list");
}

/// `rel` paths use `/`, whatever the system: they are `builtin:` names.
fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        if entry.path().is_dir() {
            walk(&entry.path(), &rel, out);
        } else if [".wav", ".flac"].iter().any(|ext| name.to_ascii_lowercase().ends_with(ext)) {
            out.push(rel);
        }
    }
}
