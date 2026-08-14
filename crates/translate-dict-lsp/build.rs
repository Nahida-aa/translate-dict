// Compute the absolute path of the built-in dictionary dir to embed and generate Rust
// source containing a literal path (include_dir! only accepts string literals, not env! values).
// The generated file lives at OUT_DIR/embedded_dict.rs and declares:
//   pub static EMBEDDED: include_dir::Dir = include_dir::include_dir!("/abs/dict");
use std::path::Path;

fn main() {
    let manifest = env!("CARGO_MANIFEST_DIR"); // crates/translate-dict-lsp
                                               // repo-root dict/: manifest -> .. (crates) -> .. (translate-dict) -> dict
                                               // NB: cannot use canonicalize() — on Windows it produces a "\\?\\D:\\..." UNC prefix,
                                               // which include_dir! rejects with "not a directory". Just join a plain path.
    let dict = Path::new(manifest).join("../../dict");
    assert!(
        dict.exists(),
        "built-in dict/ directory not found at {}",
        dict.display()
    );
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let generated = format!(
        "pub static EMBEDDED: include_dir::Dir = include_dir::include_dir!({:?});\n",
        dict.display().to_string()
    );
    std::fs::write(out.join("embedded_dict.rs"), generated).unwrap();
    println!("cargo:rerun-if-changed=../../dict");
}
