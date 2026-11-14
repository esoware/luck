/// The names under `[dependencies]` in this crate's manifest.
fn dependency_names() -> Vec<&'static str> {
    let manifest = include_str!("../../Cargo.toml");
    let (_, after_header) =
        manifest.split_once("\n[dependencies]").expect("the manifest lists its dependencies");
    let table = after_header.split_once("\n[").map_or(after_header, |(table, _)| table);
    table.lines().filter_map(|line| line.split_once('=')).map(|(name, _)| name.trim()).collect()
}

#[test]
fn depends_on_nothing_but_luck_token() {
    // Every embedder links this crate, so it reads no files and pulls in no
    // third-party code.
    assert_eq!(dependency_names(), ["luck_token"]);
}
