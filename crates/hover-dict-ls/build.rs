// 计算要嵌入二进制的内置词库目录的绝对路径，生成一段含字面量路径的 Rust
// 源码（include_dir! 只接受字符串字面量，无法直接吃 env! 的变量）。
// 生成的文件在 OUT_DIR/embedded_dict.rs，声明：
//   pub static EMBEDDED: include_dir::Dir = include_dir::include_dir!("/abs/dict");
use std::path::Path;

fn main() {
    let manifest = env!("CARGO_MANIFEST_DIR"); // crates/hover-dict-ls
                                               // 仓库根 dict/：manifest -> .. (crates) -> .. (hover-dict) -> dict
    let dict = Path::new(manifest)
        .join("../../dict")
        .canonicalize()
        .expect("built-in dict/ directory not found");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let generated = format!(
        "pub static EMBEDDED: include_dir::Dir = include_dir::include_dir!({:?});\n",
        dict.display().to_string()
    );
    std::fs::write(out.join("embedded_dict.rs"), generated).unwrap();
    println!("cargo:rerun-if-changed=../../dict");
}
