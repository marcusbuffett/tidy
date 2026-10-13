use oxc_resolver::{ResolveOptions, Resolver, TsconfigDiscovery, TsconfigOptions, TsconfigReferences};
use std::path::PathBuf;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let tsconfig = PathBuf::from(&args[1]);
    let dir = PathBuf::from(&args[2]);
    let spec = &args[3];
    let base_options = ResolveOptions {
        extensions: [".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs", ".json", ".node", ".d.ts"].iter().map(|s| s.to_string()).collect(),
        extension_alias: vec![
            (".js".into(), vec![".ts".into(), ".tsx".into(), ".js".into(), ".jsx".into()]),
            (".jsx".into(), vec![".tsx".into(), ".jsx".into()]),
            (".mjs".into(), vec![".mts".into(), ".mjs".into()]),
            (".cjs".into(), vec![".cts".into(), ".cjs".into()]),
        ],
        condition_names: ["types", "import", "require", "node", "bun", "browser", "default", "development"].iter().map(|s| s.to_string()).collect(),
        main_fields: ["types", "source", "module", "main"].iter().map(|s| s.to_string()).collect(),
        enforce_extension: oxc_resolver::EnforceExtension::Auto,
        builtin_modules: true,
        symlinks: true,
        ..ResolveOptions::default()
    };
    let base = Resolver::new(base_options.clone());
    let mut opts = base_options.clone();
    opts.tsconfig = Some(TsconfigDiscovery::Manual(TsconfigOptions { config_file: tsconfig, references: TsconfigReferences::Auto }));
    let r = if std::env::var("CLONE").is_ok() { base.clone_with_options(opts) } else { Resolver::new(opts) };
    println!("{:?}", r.resolve(&dir, spec).map(|r| r.full_path()));
    // globset check
    for pat in ["**/{,*.}{spec,test}.[jt]s{,x}", "**/*.{spec,test}.{,c,m}[jt]s{,x}"] {
        let g = globset::GlobBuilder::new(pat).literal_separator(true).build().unwrap().compile_matcher();
        println!("{pat}: {} {}", g.is_match("src/a/b.test.ts"), g.is_match("src/a/test.ts"));
    }
}
