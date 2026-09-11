//! `web/dist` is gitignored and only exists once `pnpm --dir web build` has run, but
//! `rust-embed` fails to compile on a missing folder. Creating it empty keeps `cargo build`
//! and `nix build` working on a fresh checkout; the server then says the UI was not built.

fn main() {
    let dist = std::path::Path::new("web/dist");
    if !dist.exists()
        && let Err(error) = std::fs::create_dir_all(dist)
    {
        println!(
            "cargo::warning=could not create {}: {error}",
            dist.display()
        );
    }
    println!("cargo::rerun-if-changed=web/dist");
}
